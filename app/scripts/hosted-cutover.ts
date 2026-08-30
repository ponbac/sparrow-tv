import { createHash, createHmac } from "node:crypto";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { readFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import {
  HOSTED_REHEARSAL_FIXTURE_SHA256,
  LEGACY_ROLLBACK_DIGEST,
  parseHostedCutoverPlan,
  parseHostedCutoverReadiness,
  parseHostedAcceptanceEvidence,
  parseHostedEndpointEvidence,
  parseHostedImageReference,
  prepareHostedCutover,
  sealHostedProductionEvidence,
  verifyHostedCutoverReadiness,
  verifyHostedRehearsal,
  type HostedCutoverPlan,
} from "./hosted-cutover-domain.ts";
import {
  projectAcceptanceCandidate,
  verifyAcceptanceApprovalHistory,
} from "./release-acceptance-domain.ts";
import {
  parseCandidateManifest,
  parseProductVersion,
  type CandidateManifest,
} from "./release-contract-domain.ts";
import {
  prepareReleaseOutput,
  readReleaseRegularFile,
  readReleasePrivateRegularFile,
  writeReleasePrivateFile,
} from "./release-filesystem.ts";

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const CONTRACT_PATH = join(REPOSITORY_ROOT, "deployment/hosted-contract.json");
const ROLLBACK_REFERENCE = `docker.io/ponbac/sparrow:0.11.4@${LEGACY_ROLLBACK_DIGEST}`;
const MAX_COMMAND_OUTPUT = 4 * 1024 * 1024;
let boundDockerContext: string | undefined;
const TRUSTED_TOOLS = {
  compose: { path: "/usr/libexec/docker/cli-plugins/docker-compose", sha256: "c57ab918abd5b05ca7e7d0f275875dd1330a695074f309dc9eab1b49efafcd4b" },
  buildx: { path: "/usr/libexec/docker/cli-plugins/docker-buildx", sha256: "a8aba78b2f36a061aaf1c060fc0197a1b02c1780591f8b764ab43065173d969e" },
} as const;

const manifestSeedSchema = z.object({ version: z.string(), tag: z.string(), commit: z.string() }).passthrough();
const githubRefSchema = z.object({ object: z.object({ sha: z.string(), type: z.string() }).strip() }).strip();
const githubRunSchema = z.object({
  id: z.number().int().positive(), run_attempt: z.number().int().positive(), head_sha: z.string(),
  event: z.string(), head_branch: z.string().nullable(), status: z.string(), conclusion: z.string().nullable(),
  name: z.string(), path: z.string(),
}).strip();
const githubRunsSchema = z.object({ workflow_runs: z.array(githubRunSchema) }).strip();
const githubCompareSchema = z.object({ status: z.enum(["ahead", "identical", "behind", "diverged"]) }).strip();
const githubIssueSchema = z.object({ state: z.literal("closed") }).strip();
const githubCommentsSchema = z.array(z.object({ body: z.string(), user: z.object({ login: z.string() }).strip(), author_association: z.string().min(1).max(64) }).strip());
const contextSchema = z.array(z.object({
  Endpoints: z.object({ docker: z.object({ Host: z.string(), SkipTLSVerify: z.boolean().optional() }).strip() }).strip(),
}).strip()).length(1);
const containerSchema = z.array(z.object({
  Id: z.string().regex(/^[0-9a-f]{64}$/u), Image: z.string().regex(/^sha256:[0-9a-f]{64}$/u),
  Config: z.object({
    Image: z.string(), Labels: z.record(z.string(), z.string()).nullable(),
    ExposedPorts: z.record(z.string(), z.unknown()).nullable().optional(),
  }).strip(),
  State: z.object({ Running: z.boolean(), StartedAt: z.string(), Health: z.object({ Status: z.string() }).optional() }).strip(),
  RestartCount: z.number().int().nonnegative(),
  NetworkSettings: z.object({ Ports: z.record(z.string(), z.unknown()).nullable(), Networks: z.record(z.string(), z.object({ NetworkID: z.string().min(1), Aliases: z.array(z.string()).nullable() }).strip()) }).strip(),
}).strip()).length(1);
const imageInspectSchema = z.array(z.object({
  Id: z.string().regex(/^sha256:[0-9a-f]{64}$/u), RepoDigests: z.array(z.string()).nullable(),
}).strip()).length(1);
const composeProjectionSchema = z.object({ name: z.string().regex(/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/u), services: z.record(z.string(), z.object({
  image: z.string(), environment: z.record(z.string(), z.string()), ports: z.array(z.object({ target: z.number().int(), published: z.union([z.string(), z.number()]).optional() }).passthrough()).optional(),
}).passthrough()) }).passthrough();

interface CliArguments {
  readonly command: string;
  readonly values: ReadonlyMap<string, string>;
}

class HostedCutoverFailure extends Error {
  readonly _tag = "HostedCutoverFailure";
}

async function main(): Promise<void> {
  const arguments_ = parseArguments(process.argv.slice(2));
  switch (arguments_.command) {
    case "prepare": await prepare(arguments_.values); return;
    case "verify-rehearsal": await verifyRehearsal(arguments_.values); return;
    case "verify-readiness": await verifyReadiness(arguments_.values); return;
    case "seal-production-evidence": await sealProductionEvidence(arguments_.values); return;
    case "record-endpoint": await recordEndpoint(arguments_.values); return;
    case "record-hosted-acceptance": await recordHostedAcceptance(arguments_.values); return;
    case "print-rehearsal-plan": await printRehearsalPlan(arguments_.values); return;
    case "start-production-observation": await startProductionObservation(arguments_.values); return;
    case "finish-production-observation": await finishProductionObservation(arguments_.values); return;
    case "snapshot-rehearsal-fixture": await snapshotRehearsalFixture(arguments_.values); return;
    case "record-route-binding": await recordRouteBinding(arguments_.values); return;
  }
  throw new HostedCutoverFailure("unknown hosted-cutover command");
}

async function recordRouteBinding(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--readiness", "--start", "--endpoint", "--caddy-backup", "--evidence-key", "--container", "--image-role", "--acknowledgement", "--output"]);
  if (required(values, "--acknowledgement") !== "owner-observed-route-to-container") throw new HostedCutoverFailure("the explicit route observation acknowledgement is required");
  const readinessSnapshot = await readJsonSnapshot(repositoryPath(required(values, "--readiness")));
  const startSnapshot = await readJsonSnapshot(repositoryPath(required(values, "--start")));
  const endpointSnapshot = await readJsonSnapshot(repositoryPath(required(values, "--endpoint")));
  const readiness = parseHostedCutoverReadiness(readinessSnapshot.value); if (!readiness.ok) throw new HostedCutoverFailure(readiness.reason);
  const key = await readEvidenceKey(repositoryPath(required(values, "--evidence-key"))); const role = required(values, "--image-role");
  if (role !== "replacement" && role !== "baseline" && role !== "rollback") throw new HostedCutoverFailure("the route image role is invalid");
  const expected = role === "replacement" ? readiness.value.replacement : { image: readiness.value.rollback.image, revision: null };
  const runtime = inspectContainer(required(values, "--container"), expected.image.reference, expected.revision, key);
  const record = { schemaVersion: 1 as const, observedAt: new Date().toISOString(), role, readinessSha256: readinessSnapshot.sha256,
    startSha256: startSnapshot.sha256, endpointSha256: endpointSnapshot.sha256,
    caddyBinding: await privateBinding("caddy", repositoryPath(required(values, "--caddy-backup")), key),
    dockerEndpointBinding: runtime.dockerEndpointBinding, containerIdentitySha256: runtime.containerIdentitySha256,
    acknowledgement: "owner-observed-route-to-container" as const };
  await writePrivateJson(repositoryPath(required(values, "--output")), { ...record, routeHmac: evidenceHmac("route-binding", record, key) });
}

async function snapshotRehearsalFixture(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--output"]);
  const fixture = await readReleaseRegularFile(join(REPOSITORY_ROOT, "scripts/hosted-rehearsal-fixture.py"));
  const digest = createHash("sha256").update(fixture).digest("hex");
  if (digest !== HOSTED_REHEARSAL_FIXTURE_SHA256) throw new HostedCutoverFailure("the committed rehearsal fixture digest is invalid");
  const target = await prepareReleaseOutput(repositoryPath(required(values, "--output")), []);
  try { await writeReleasePrivateFile(target, fixture.toString("utf8")); } finally { await target.close(); }
}

async function startProductionObservation(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--readiness", "--evidence-key", "--output"]);
  const readiness = await readJsonSnapshot(repositoryPath(required(values, "--readiness")));
  const key = await readEvidenceKey(repositoryPath(required(values, "--evidence-key")));
  const record = { schemaVersion: 1 as const, startedAt: new Date().toISOString(), monotonicStarted: (await readBootUptimeMilliseconds()).toString(),
    bootIdBinding: privateBindingBytes("boot-id", await readBootId(), key), readinessSha256: readiness.sha256 };
  await writePrivateJson(repositoryPath(required(values, "--output")), { ...record, startHmac: evidenceHmac("cutover-start", record, key) });
}

async function finishProductionObservation(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--start", "--route-binding", "--result", "--failure", "--incident-reference", "--evidence-key", "--output"]);
  const startSnapshot = await readJsonSnapshot(repositoryPath(required(values, "--start")));
  const start = z.object({ schemaVersion: z.literal(1), startedAt: z.string().datetime({ offset: true }), monotonicStarted: z.string().regex(/^\d+$/u), bootIdBinding: z.string(), readinessSha256: z.string().regex(/^[0-9a-f]{64}$/u), startHmac: z.string().regex(/^[0-9a-f]{64}$/u) }).strict()
    .safeParse(startSnapshot.value);
  const key = await readEvidenceKey(repositoryPath(required(values, "--evidence-key")));
  if (!start.success) throw new HostedCutoverFailure("the observation start is invalid");
  const { startHmac, ...startFields } = start.data;
  const now = await readBootUptimeMilliseconds();
  if (startHmac !== evidenceHmac("cutover-start", startFields, key) || start.data.bootIdBinding !== privateBindingBytes("boot-id", await readBootId(), key) || now < BigInt(start.data.monotonicStarted)) throw new HostedCutoverFailure("the observation start was altered or belongs to another boot");
  const selection = z.discriminatedUnion("result", [
    z.object({ result: z.literal("passed"), failure: z.literal(""), incidentReference: z.literal("") }).strict(),
    z.object({ result: z.literal("registry-failed"), failure: z.literal("registry-pull-failed"), incidentReference: z.literal("") }).strict(),
    z.object({ result: z.literal("failed"), failure: z.literal("service-recreate-failed"), incidentReference: z.literal("") }).strict(),
    z.object({ result: z.literal("failed-after-start"), failure: z.enum(["healthcheck-failed", "topology-mismatch", "runtime-crash", "hosted-endpoint-verification-failed"]), incidentReference: z.literal("") }).strict(),
    z.object({ result: z.literal("recovery-failed"), failure: z.enum(["service-recreate-failed", "healthcheck-failed", "topology-mismatch", "runtime-crash", "hosted-endpoint-verification-failed"]), incidentReference: z.string().regex(/^INC-[A-Z0-9-]{4,64}$/u) }).strict(),
  ]).safeParse({ result: required(values, "--result"), failure: present(values, "--failure"), incidentReference: present(values, "--incident-reference") });
  if (!selection.success) throw new HostedCutoverFailure("the observation outcome arguments are inconsistent");
  const elapsedMilliseconds = Number(now - BigInt(start.data.monotonicStarted));
  const completedAt = new Date(Date.parse(start.data.startedAt) + elapsedMilliseconds).toISOString();
  const result = selection.data.result;
  const outage = result === "registry-failed" ? { startedAt: null, restoredAt: null }
    : result === "recovery-failed" ? { startedAt: start.data.startedAt, restoredAt: null }
    : { startedAt: start.data.startedAt, restoredAt: completedAt };
  const outcome = result === "passed" ? { result } : result === "recovery-failed" ? selection.data : { result, failure: selection.data.failure };
  const routePath = present(values, "--route-binding");
  let routeBindingSha256: string | null = null;
  if (result === "recovery-failed") {
    if (routePath !== "") throw new HostedCutoverFailure("an incident-open observation cannot claim a route binding");
  } else {
    if (routePath === "") throw new HostedCutoverFailure("a completed recovery requires a route binding");
    const routeSnapshot = await readJsonSnapshot(repositoryPath(routePath));
    const routeTime = z.object({ observedAt: z.string().datetime({ offset: true }), startSha256: z.string() }).passthrough().safeParse(routeSnapshot.value);
    if (!routeTime.success || routeTime.data.startSha256 !== startSnapshot.sha256 || Date.parse(routeTime.data.observedAt) < Date.parse(start.data.startedAt) || Date.parse(routeTime.data.observedAt) > Date.parse(completedAt)) throw new HostedCutoverFailure("the route observation is outside the production window");
    routeBindingSha256 = routeSnapshot.sha256;
  }
  const event = { schemaVersion: 1 as const, readinessSha256: start.data.readinessSha256, routeBindingSha256, startedAt: start.data.startedAt,
    completedAt, elapsedMilliseconds, outage, ...outcome };
  await writePrivateJson(repositoryPath(required(values, "--output")), { ...event, eventHmac: evidenceHmac("cutover-event", event, key) });
}

async function printRehearsalPlan(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--plan"]);
  const parsed = parseHostedCutoverPlan(await readJson(repositoryPath(required(values, "--plan"))));
  if (!parsed.ok) throw new HostedCutoverFailure(parsed.reason);
  process.stdout.write(JSON.stringify({
    candidateImage: parsed.value.replacement.image.reference,
    candidateRevision: parsed.value.replacement.revision,
    rollbackImage: parsed.value.rollback.image.reference,
    fixtureImage: "docker.io/library/python:3.13.15-alpine3.24@sha256:540c7d91f98ff6880174c40e99067bf5941eb54d818a7a5e094d188b196a934d",
    fixtureScriptSha256: await fileSha256(join(REPOSITORY_ROOT, "scripts/hosted-rehearsal-fixture.py")),
  }));
}

async function prepare(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, [
    "--hosted-acceptance", "--baseline-compose", "--candidate-compose", "--environment-backup",
    "--caddy-backup", "--evidence-key", "--container", "--replacement-image", "--output",
  ]);
  const paths = evidencePaths(values);
  const key = await readEvidenceKey(paths.evidenceKey);
  const hostedSnapshot = await readJsonSnapshot(paths.hostedAcceptance);
  const hostedAcceptance = hostedSnapshot.value;
  readHostedAcceptanceReference(hostedAcceptance);
  const replacementReference = required(values, "--replacement-image");
  const contractSnapshot = await readJsonSnapshot(CONTRACT_PATH);
  const facts = await observeTrustedFacts({
    baselineCompose: paths.baselineCompose, candidateCompose: paths.candidateCompose,
    environment: paths.environment, caddy: paths.caddy, key,
    container: required(values, "--container"), replacementReference,
  });
  const plan = prepareHostedCutover({
    contract: contractSnapshot.value, contractSha256: contractSnapshot.sha256,
    hostedAcceptance, hostedAcceptanceSha256: hostedSnapshot.sha256,
    preparedAt: new Date().toISOString(), trustedFacts: facts,
  });
  if (!plan.ok) throw new HostedCutoverFailure(plan.reason);
  await writePrivateJson(paths.output, plan.value);
  process.stdout.write(`prepared=${plan.value.replacement.image.digest}\n`);
}

async function verifyRehearsal(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--plan", "--observation", "--environment-backup", "--evidence-key", "--output"]);
  const planPath = repositoryPath(required(values, "--plan"));
  const observation = await readJson(repositoryPath(required(values, "--observation")));
  const key = await readEvidenceKey(repositoryPath(required(values, "--evidence-key")));
  const environmentBinding = await privateBinding("environment", repositoryPath(required(values, "--environment-backup")), key);
  const enriched = z.object({ schemaVersion: z.unknown(), rehearsal: z.unknown(), recordedAt: z.unknown(), dockerContextClass: z.unknown(), fixture: z.unknown(), steps: z.unknown() }).strict().safeParse(observation);
  if (!enriched.success) throw new HostedCutoverFailure("the rehearsal observation is invalid");
  const verified = verifyHostedRehearsal({
    plan: await readJson(planPath), observation: { ...enriched.data, environmentBinding }, verifiedAt: new Date().toISOString(),
  });
  if (!verified.ok) throw new HostedCutoverFailure(verified.reason);
  await writePrivateJson(repositoryPath(required(values, "--output")), verified.value);
  process.stdout.write(`rehearsed=${verified.value.rollbackImageDigest}\n`);
}

async function verifyReadiness(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, [
    "--plan", "--rehearsal", "--hosted-acceptance", "--candidate", "--acceptance-verdict", "--baseline-compose",
    "--candidate-compose", "--environment-backup", "--caddy-backup", "--evidence-key", "--container", "--output",
  ]);
  const paths = evidencePaths(values);
  const planPath = repositoryPath(required(values, "--plan"));
  const rehearsalPath = repositoryPath(required(values, "--rehearsal"));
  const candidatePath = repositoryPath(required(values, "--candidate"));
  const verdictPath = repositoryPath(required(values, "--acceptance-verdict"));
  const planSnapshot = await readJsonSnapshot(planPath); const planInput = planSnapshot.value;
  const plan = parsePlanSeed(planInput);
  const candidateSnapshot = await readJsonSnapshot(candidatePath); const candidateInput = candidateSnapshot.value;
  const candidate = parseManifest(candidateInput);
  const verdictSnapshot = await readJsonSnapshot(verdictPath); const verdictSha256 = verdictSnapshot.sha256;
  const rehearsalSnapshot = await readJsonSnapshot(rehearsalPath);
  const hostedSnapshot = await readJsonSnapshot(paths.hostedAcceptance);
  const key = await readEvidenceKey(paths.evidenceKey);
  const facts = await observeTrustedFacts({
    baselineCompose: paths.baselineCompose, candidateCompose: paths.candidateCompose,
    environment: paths.environment, caddy: paths.caddy, key,
    container: required(values, "--container"), replacementReference: plan.replacement.image.reference,
  });
  const live = readGithubReadiness(candidate, plan, verdictSnapshot.value, verdictSha256, hostedSnapshot.sha256);
  const baselineObservation = {
    schemaVersion: 1, recordedAt: new Date().toISOString(), image: facts.baseline.image,
    containerIdentitySha256: facts.baseline.containerIdentitySha256,
    topology: { ...plan.topology, dockerContextClass: facts.baseline.dockerContextClass },
    configuration: facts.configuration, backupsReadable: facts.backupsReadable,
    replacementRegistryAvailable: facts.replacementRegistryAvailable,
    rollbackRegistryAvailable: facts.rollbackRegistryAvailable,
  };
  const verified = verifyHostedCutoverReadiness({
    plan: planInput, planSha256: planSnapshot.sha256, rehearsal: rehearsalSnapshot.value,
    hostedAcceptance: hostedSnapshot.value, hostedAcceptanceSha256: hostedSnapshot.sha256,
    rehearsalSha256: rehearsalSnapshot.sha256, candidateManifest: candidateInput,
    candidateManifestSha256: candidateSnapshot.sha256, acceptanceVerdict: verdictSnapshot.value,
    acceptanceVerdictSha256: verdictSha256, workflowEvidence: live.workflows,
    baselineObservation, masterCommit: live.masterCommit, verifiedAt: new Date().toISOString(),
  });
  if (!verified.ok) throw new HostedCutoverFailure(verified.reason);
  await writePrivateJson(paths.output, verified.value);
  process.stdout.write(`ready=${verified.value.masterCommit}\n`);
}

async function sealProductionEvidence(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--readiness", "--event", "--endpoint", "--route-binding", "--baseline-compose", "--candidate-compose", "--environment-backup", "--caddy-backup", "--evidence-key", "--container", "--output"]);
  const readinessPath = repositoryPath(required(values, "--readiness"));
  const readinessSnapshot = await readJsonSnapshot(readinessPath); const readinessInput = readinessSnapshot.value;
  const readiness = parseHostedCutoverReadiness(readinessInput);
  if (!readiness.ok) throw new HostedCutoverFailure(readiness.reason);
  const eventBase = z.object({
    schemaVersion: z.literal(1), readinessSha256: z.string(), startedAt: z.string(), completedAt: z.string(), elapsedMilliseconds: z.number().int().nonnegative(),
    outage: z.object({ startedAt: z.string().nullable(), restoredAt: z.string().nullable() }).strict(),
    eventHmac: z.string().regex(/^[0-9a-f]{64}$/u),
  });
  const eventSnapshot = await readJsonSnapshot(repositoryPath(required(values, "--event")));
  const event = z.discriminatedUnion("result", [
    eventBase.extend({ result: z.literal("passed"), routeBindingSha256: z.string().regex(/^[0-9a-f]{64}$/u) }).strict(),
    eventBase.extend({ result: z.literal("registry-failed"), routeBindingSha256: z.string().regex(/^[0-9a-f]{64}$/u), failure: z.literal("registry-pull-failed") }).strict(),
    eventBase.extend({ result: z.literal("failed"), routeBindingSha256: z.string().regex(/^[0-9a-f]{64}$/u), failure: z.literal("service-recreate-failed") }).strict(),
    eventBase.extend({ result: z.literal("failed-after-start"), routeBindingSha256: z.string().regex(/^[0-9a-f]{64}$/u), failure: z.enum(["healthcheck-failed", "topology-mismatch", "runtime-crash", "hosted-endpoint-verification-failed"]) }).strict(),
    eventBase.extend({ result: z.literal("recovery-failed"), routeBindingSha256: z.null(), failure: z.enum(["service-recreate-failed", "healthcheck-failed", "topology-mismatch", "runtime-crash", "hosted-endpoint-verification-failed"]), incidentReference: z.string().regex(/^INC-[A-Z0-9-]{4,64}$/u) }).strict(),
  ]).safeParse(eventSnapshot.value);
  const key = await readEvidenceKey(repositoryPath(required(values, "--evidence-key")));
  if (!event.success) throw new HostedCutoverFailure("the production event is invalid");
  const { eventHmac, ...eventFields } = event.data;
  if (eventHmac !== evidenceHmac("cutover-event", eventFields, key) ||
    Date.parse(event.data.completedAt) - Date.parse(event.data.startedAt) !== event.data.elapsedMilliseconds) {
    throw new HostedCutoverFailure("the production event timing proof is invalid");
  }
  const caddy = await readReleasePrivateRegularFile(repositoryPath(required(values, "--caddy-backup")));
  validateCaddyProjection(caddy);
  const environmentBytes = await readReleasePrivateRegularFile(repositoryPath(required(values, "--environment-backup")));
  const shared = {
    environmentBinding: privateBindingBytes("environment", environmentBytes, key),
    caddyBinding: privateBindingBytes("caddy", caddy, key),
  };
  const isCandidate = event.data.result === "passed";
  const baselineComposeBytes = await readReleasePrivateRegularFile(repositoryPath(required(values, "--baseline-compose")));
  const candidateComposeBytes = await readReleasePrivateRegularFile(repositoryPath(required(values, "--candidate-compose")));
  const compose = await verifyComposeProjection(baselineComposeBytes, candidateComposeBytes, environmentBytes, readiness.value.replacement.image.reference);
  if (privateBindingBytes("baseline-service", Buffer.from(compose.baselineHash), key) !== readiness.value.configuration.baselineServiceBinding ||
    privateBindingBytes("candidate-service", Buffer.from(compose.candidateHash), key) !== readiness.value.configuration.candidateServiceBinding) {
    throw new HostedCutoverFailure("the rendered Compose service hashes differ from readiness");
  }
  const reference = isCandidate ? readiness.value.replacement.image.reference : readiness.value.rollback.image.reference;
  const revision = isCandidate ? readiness.value.replacement.revision : null;
  const inspectedRuntime = event.data.result === "recovery-failed" ? undefined
    : inspectContainer(required(values, "--container"), reference, revision, key);
  const runtime = inspectedRuntime === undefined ? undefined : (({ composeProject: _project, composeConfigHash: _hash, ...value }) => {
    void _project; void _hash; return value;
  })(inspectedRuntime);
  if (inspectedRuntime !== undefined && (inspectedRuntime.composeProject !== compose.project ||
    inspectedRuntime.composeConfigHash !== (isCandidate ? compose.candidateHash : compose.baselineHash))) throw new HostedCutoverFailure("the live runtime does not match the exact rendered Compose service");
  const endpointPath = present(values, "--endpoint"); const routePath = present(values, "--route-binding");
  if (event.data.result === "recovery-failed" && (endpointPath !== "" || routePath !== "")) throw new HostedCutoverFailure("incident-open evidence cannot claim endpoint or route recovery");
  if (event.data.result !== "recovery-failed" && (endpointPath === "" || routePath === "")) throw new HostedCutoverFailure("completed recovery requires endpoint and route evidence");
  const endpointSnapshot = endpointPath === "" ? undefined : await readJsonSnapshot(repositoryPath(endpointPath));
  const endpoint = endpointSnapshot?.value;
  const routeSnapshot = routePath === "" ? undefined : await readJsonSnapshot(repositoryPath(routePath));
  const route = z.object({ schemaVersion: z.literal(1), observedAt: z.string().datetime({ offset: true }), role: z.enum(["replacement", "baseline", "rollback"]), readinessSha256: z.string(), startSha256: z.string(), endpointSha256: z.string(), caddyBinding: z.string(), dockerEndpointBinding: z.string(), containerIdentitySha256: z.string(), acknowledgement: z.literal("owner-observed-route-to-container"), routeHmac: z.string() }).strict()
    .safeParse(routeSnapshot?.value);
  if (route.success && route.data.caddyBinding !== shared.caddyBinding) throw new HostedCutoverFailure("the route binding uses another Caddy snapshot");
  if (event.data.result !== "recovery-failed") {
    if (!route.success || runtime === undefined) throw new HostedCutoverFailure("the explicit route binding is invalid");
    const { routeHmac, ...routeFields } = route.data; const expectedRole = isCandidate ? "replacement" : event.data.result === "registry-failed" ? "baseline" : "rollback";
    if (routeHmac !== evidenceHmac("route-binding", routeFields, key) || route.data.role !== expectedRole ||
      route.data.readinessSha256 !== readinessSnapshot.sha256 || routeSnapshot === undefined || event.data.routeBindingSha256 !== routeSnapshot.sha256 ||
      endpointSnapshot === undefined || route.data.endpointSha256 !== endpointSnapshot.sha256 ||
      Date.parse(route.data.observedAt) < Date.parse(event.data.startedAt) || Date.parse(route.data.observedAt) > Date.parse(event.data.completedAt) ||
      route.data.dockerEndpointBinding !== runtime.dockerEndpointBinding || route.data.containerIdentitySha256 !== runtime.containerIdentitySha256) {
      throw new HostedCutoverFailure("the endpoint is not explicitly bound to the inspected runtime");
    }
  }
  const configuration = {
    composeBinding: privateBindingBytes(isCandidate ? "candidate-compose" : "baseline-compose", isCandidate ? candidateComposeBytes : baselineComposeBytes, key),
    ...shared,
  };
  const base = { schemaVersion: 1, readinessSha256: event.data.readinessSha256, startedAt: event.data.startedAt,
    completedAt: event.data.completedAt, outage: event.data.outage };
  let attempt: unknown;
  if (event.data.result === "passed" && runtime !== undefined && endpoint !== undefined) attempt = { result: "passed", deployed: { runtime, configuration }, endpoint };
  else if (event.data.result === "registry-failed" && runtime !== undefined && endpoint !== undefined) attempt = { result: "registry-failed", failure: "registry-pull-failed", deployed: null,
    rollback: { performed: false, reason: "baseline-unmodified", runtime, configuration, endpoint } };
  else if (event.data.result === "failed" && runtime !== undefined && endpoint !== undefined) attempt = { result: "failed", failure: "service-recreate-failed", deployed: null,
    rollback: { performed: true, runtime, configuration, endpoint } };
  else if (event.data.result === "failed-after-start" && runtime !== undefined && endpoint !== undefined) attempt = { result: "failed-after-start", failure: event.data.failure,
    rollback: { performed: true, runtime, configuration, endpoint } };
  else if (event.data.result === "recovery-failed") attempt = { result: "recovery-failed", failure: event.data.failure,
    incident: { opened: true, reference: event.data.incidentReference } };
  else throw new HostedCutoverFailure("the observed runtime is unavailable for the selected outcome");
  const sealed = sealHostedProductionEvidence({
    readiness: readinessInput, readinessSha256: readinessSnapshot.sha256, observation: { ...base, attempt }, sealedAt: new Date().toISOString(),
  });
  if (!sealed.ok) throw new HostedCutoverFailure(sealed.reason);
  await writePrivateJson(repositoryPath(required(values, "--output")), sealed.value);
  process.stdout.write(`sealed=${sealed.value.verdict}\n`);
}

async function recordEndpoint(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--input", "--output"]);
  const parsed = parseHostedEndpointEvidence(await readJson(repositoryPath(required(values, "--input"))));
  if (!parsed.ok) throw new HostedCutoverFailure(parsed.reason);
  await writePrivateJson(repositoryPath(required(values, "--output")), parsed.value);
  process.stdout.write("endpoint=recorded\n");
}

async function recordHostedAcceptance(values: ReadonlyMap<string, string>): Promise<void> {
  requireExactFlags(values, ["--input", "--output"]);
  const parsed = parseHostedAcceptanceEvidence(await readJson(repositoryPath(required(values, "--input"))));
  if (!parsed.ok) throw new HostedCutoverFailure(parsed.reason);
  await writePrivateJson(repositoryPath(required(values, "--output")), parsed.value);
  process.stdout.write("hosted-acceptance=recorded\n");
}

function evidencePaths(values: ReadonlyMap<string, string>) {
  return {
    hostedAcceptance: repositoryPath(optional(values, "--hosted-acceptance") ?? "release-acceptance/hosted/hosted-acceptance.json"),
    baselineCompose: repositoryPath(required(values, "--baseline-compose")),
    candidateCompose: repositoryPath(required(values, "--candidate-compose")),
    environment: repositoryPath(required(values, "--environment-backup")),
    caddy: repositoryPath(required(values, "--caddy-backup")),
    evidenceKey: repositoryPath(required(values, "--evidence-key")),
    output: repositoryPath(required(values, "--output")),
  };
}

async function observeTrustedFacts(input: {
  readonly baselineCompose: string; readonly candidateCompose: string; readonly environment: string;
  readonly caddy: string; readonly key: Buffer; readonly container: string; readonly replacementReference: string;
}) {
  const caddy = await readReleasePrivateRegularFile(input.caddy);
  validateCaddyProjection(caddy);
  const inspectedBaseline = inspectBaselineContainer(input.container, input.key);
  const baselineCompose = await readReleasePrivateRegularFile(input.baselineCompose);
  const candidateCompose = await readReleasePrivateRegularFile(input.candidateCompose);
  const environment = await readReleasePrivateRegularFile(input.environment);
  const compose = await verifyComposeProjection(baselineCompose, candidateCompose, environment, input.replacementReference);
  if (inspectedBaseline.composeProject !== compose.project || inspectedBaseline.composeConfigHash !== compose.baselineHash) throw new HostedCutoverFailure("the baseline runtime does not match rendered Compose");
  const { composeProject: _project, composeConfigHash: _hash, ...baseline } = inspectedBaseline;
  void _project; void _hash;
  requireRegistryAvailable(input.replacementReference);
  requireRegistryAvailable(ROLLBACK_REFERENCE);
  const replacementImage = parseHostedImageReference(input.replacementReference);
  if (!replacementImage.ok) throw new HostedCutoverFailure(replacementImage.reason);
  return {
    replacementImage: replacementImage.value,
    configuration: {
      baselineComposeBinding: privateBindingBytes("baseline-compose", baselineCompose, input.key),
      candidateComposeBinding: privateBindingBytes("candidate-compose", candidateCompose, input.key),
      baselineServiceBinding: privateBindingBytes("baseline-service", Buffer.from(compose.baselineHash), input.key),
      candidateServiceBinding: privateBindingBytes("candidate-service", Buffer.from(compose.candidateHash), input.key),
      environmentBinding: privateBindingBytes("environment", environment, input.key),
      caddyBinding: privateBindingBytes("caddy", caddy, input.key),
    },
    baseline, backupsReadable: true as const, replacementRegistryAvailable: true as const, rollbackRegistryAvailable: true as const,
  };
}

async function verifyComposeProjection(baselineBytes: Buffer, candidateBytes: Buffer, environmentBytes: Buffer, replacement: string) {
  const directory = await mkdtemp(join(tmpdir(), "sparrow-compose-snapshot-"));
  const baselinePath = join(directory, "baseline.yml"); const candidatePath = join(directory, "candidate.yml"); const environmentPath = join(directory, "environment");
  await writeFile(baselinePath, baselineBytes, { mode: 0o600, flag: "wx" });
  await writeFile(candidatePath, candidateBytes, { mode: 0o600, flag: "wx" });
  await writeFile(environmentPath, environmentBytes, { mode: 0o600, flag: "wx" });
  try {
  const baseline = composeProjectionSchema.safeParse(parseJson(baselineBytes.toString("utf8")));
  const candidate = composeProjectionSchema.safeParse(parseJson(candidateBytes.toString("utf8")));
  if (!baseline.success || !candidate.success) throw new HostedCutoverFailure("Compose returned an invalid sanitized projection");
  if (hasForbiddenComposeDependency(baseline.data) || hasForbiddenComposeDependency(candidate.data)) throw new HostedCutoverFailure("rendered Compose contains an external file/build dependency");
  const baselineService = baseline.data.services.sparrow; const candidateService = candidate.data.services.sparrow;
  const environment = parseStrictDotenv(environmentBytes);
  const target = (service: typeof baselineService) => service?.ports?.some((port) => port.target === 33733) ?? false;
  if (baselineService === undefined || candidateService === undefined || !target(baselineService) || !target(candidateService) ||
    baselineService.image !== ROLLBACK_REFERENCE || candidateService.image !== replacement) throw new HostedCutoverFailure("Compose does not project the fixed immutable Sparrow images and topology");
  const ordered = (record: Record<string, string>) => Object.fromEntries(Object.entries(record).sort(([left], [right]) => left.localeCompare(right)));
  if (JSON.stringify(ordered(environment)) !== JSON.stringify(ordered(baselineService.environment)) || JSON.stringify(ordered(environment)) !== JSON.stringify(ordered(candidateService.environment))) {
    throw new HostedCutoverFailure("the environment backup does not reproduce rendered Sparrow environment");
  }
  const sanitize = (project: z.infer<typeof composeProjectionSchema>) => ({ ...project, services: { ...project.services,
    sparrow: project.services.sparrow === undefined ? undefined : { ...project.services.sparrow, image: "<immutable-image>" } } });
  if (JSON.stringify(sanitize(baseline.data)) !== JSON.stringify(sanitize(candidate.data))) {
    throw new HostedCutoverFailure("candidate Compose changes more than the intended immutable image");
  }
  if (baseline.data.name !== candidate.data.name) throw new HostedCutoverFailure("Compose project identity changed");
  const hash = (path: string) => run("compose", ["--project-name", baseline.data.name, "--env-file", environmentPath, "--file", path, "config", "--hash", "sparrow"], "hash rendered Compose service").trim();
  return { project: baseline.data.name, baselineHash: hash(baselinePath), candidateHash: hash(candidatePath) };
  } finally { await rm(directory, { recursive: true, force: true }); }
}

function hasForbiddenComposeDependency(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(hasForbiddenComposeDependency);
  if (typeof value === "string") return value.includes("${");
  if (value === null || typeof value !== "object") return false;
  return Object.entries(value).some(([key, nested]) => ["env_file", "label_file", "include", "build", "configs", "secrets", "extends"].includes(key) || hasForbiddenComposeDependency(nested));
}

function validateCaddyProjection(bytes: Buffer): void {
  const parsed = z.object({ schemaVersion: z.literal(1), routes: z.tuple([
    z.object({ host: z.literal("tv.ponbac.xyz"), upstream: z.literal("sparrow:33733") }).strict(),
  ]) }).strict().safeParse(parseJson(bytes.toString("utf8")));
  if (!parsed.success) throw new HostedCutoverFailure("the sanitized Caddy projection is not the exact unchanged public route");
}

function parseStrictDotenv(bytes: Buffer): Record<string, string> {
  const result: Record<string, string> = {};
  const text = bytes.toString("utf8");
  if (text.includes("\0") || text.includes("\r")) throw new HostedCutoverFailure("the environment backup is malformed");
  for (const line of text.split("\n")) {
    if (line === "") continue;
    const match = /^([A-Z][A-Z0-9_]*)=([^`$\n]*)$/u.exec(line);
    if (match === null || match[1] === undefined || match[2] === undefined || Object.hasOwn(result, match[1])) throw new HostedCutoverFailure("the environment backup is malformed or duplicated");
    result[match[1]] = match[2];
  }
  return Object.fromEntries(Object.entries(result).sort(([left], [right]) => left.localeCompare(right)));
}

function inspectBaselineContainer(container: string, key: Buffer) {
  return inspectContainer(container, ROLLBACK_REFERENCE, null, key);
}

function inspectContainer(container: string, reference: string, expectedRevision: string | null, key: Buffer) {
  if (!/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/u.test(container)) throw new HostedCutoverFailure("the container selector is invalid");
  const contextName = run("docker", ["context", "show"], "inspect Docker context").trim();
  if (contextName.length === 0 || contextName.length > 128) throw new HostedCutoverFailure("the Docker context name is invalid");
  boundDockerContext = contextName;
  const context = contextSchema.safeParse(parseJson(run("docker", ["context", "inspect", contextName], "inspect Docker context")));
  if (!context.success) throw new HostedCutoverFailure("Docker returned an invalid context");
  const endpoint = context.data[0]?.Endpoints.docker;
  if (endpoint === undefined) throw new HostedCutoverFailure("Docker returned no endpoint");
  const dockerContextClass = endpoint.Host.startsWith("unix://") ? "local-unix" as const
    : endpoint.Host.startsWith("ssh://") ? "remote-ssh" as const : undefined;
  if (dockerContextClass === undefined) throw new HostedCutoverFailure("the Docker context is not a supported authenticated transport");
  const inspected = containerSchema.safeParse(parseJson(run("docker", ["container", "inspect", container], "inspect Sparrow container")));
  const image = imageInspectSchema.safeParse(parseJson(run("docker", ["image", "inspect", reference], "inspect immutable image")));
  const value = inspected.success ? inspected.data[0] : undefined;
  const imageValue = image.success ? image.data[0] : undefined;
  const binding = parseHostedImageReference(reference);
  const repository = reference.slice(0, reference.lastIndexOf("@")).replace(/:[^/:]+$/u, "");
  const normalizedDigest = binding.ok ? `${repository}@${binding.value.digest}` : "";
  const labels = value?.Config.Labels;
  const project = labels?.["com.docker.compose.project"];
  const configHash = labels?.["com.docker.compose.config-hash"];
  const attachments = value === undefined ? [] : Object.entries(value.NetworkSettings.Networks).map(([name, network]) => ({ name,
    id: network.NetworkID, aliases: [...(network.Aliases ?? [])].sort() })).sort((left, right) => left.name.localeCompare(right.name));
  if (value === undefined || imageValue === undefined || value.Image !== imageValue.Id ||
    !imageValue.RepoDigests?.includes(normalizedDigest) || value.Config.Labels?.["com.docker.compose.service"] !== "sparrow" ||
    value.State.Running !== true || (value.State.Health !== undefined && value.State.Health.Status !== "healthy") ||
    project === undefined || !/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/u.test(project) || configHash === undefined || !/^[0-9a-f]{64}$/u.test(configHash) ||
    !attachments.some((attachment) => attachment.aliases.includes("sparrow")) ||
    (expectedRevision !== null && value.Config.Labels?.["org.opencontainers.image.revision"] !== expectedRevision) ||
    !(value.Config.ExposedPorts?.["33733/tcp"] !== undefined || value.NetworkSettings.Ports?.["33733/tcp"] !== undefined)) {
    throw new HostedCutoverFailure("the running baseline is not the exact Compose service and rollback image");
  }
  const imageBinding = parseHostedImageReference(reference);
  if (!imageBinding.ok) throw new HostedCutoverFailure(imageBinding.reason);
  return {
    image: imageBinding.value, revision: expectedRevision,
    composeProject: project, composeConfigHash: configHash,
    dockerEndpointBinding: privateBindingBytes("docker-endpoint", Buffer.from(endpoint.Host), key),
    runtimeTopologyBinding: privateBindingBytes("runtime-topology", Buffer.from(JSON.stringify({
      project, networks: attachments, containerPort: 33733,
    })), key),
    lifecycleBinding: privateBindingBytes("runtime-lifecycle", Buffer.from(JSON.stringify({ startedAt: value.State.StartedAt, restartCount: value.RestartCount })), key),
    containerIdentitySha256: createHmac("sha256", key).update("docker-runtime\0").update(endpoint.Host).update("\0").update(value.Id).digest("hex"),
    serviceName: "sparrow" as const, containerPort: 33733 as const, dockerContextClass,
  };
}

function requireRegistryAvailable(reference: string): void {
  run("buildx", ["imagetools", "inspect", reference], "verify immutable image availability");
}

function readGithubReadiness(candidate: CandidateManifest, plan: HostedCutoverPlan, verdict: unknown, verdictSha256: string, hostedAcceptanceSha256: string) {
  const masterRef = githubRefSchema.safeParse(ghJson(`repos/${candidate.repository}/git/ref/heads/master`));
  const tagRef = githubRefSchema.safeParse(ghJson(`repos/${candidate.repository}/git/ref/tags/${encodeURIComponent(candidate.tag)}`));
  if (!masterRef.success || masterRef.data.object.type !== "commit" || !tagRef.success || tagRef.data.object.type !== "commit" ||
    tagRef.data.object.sha !== masterRef.data.object.sha) throw new HostedCutoverFailure("the release tag is not current master");
  const masterCommit = masterRef.data.object.sha;
  const compare = githubCompareSchema.safeParse(ghJson(`repos/${candidate.repository}/compare/${plan.replacement.revision}...${masterCommit}`));
  if (!compare.success || (compare.data.status !== "ahead" && compare.data.status !== "identical")) {
    throw new HostedCutoverFailure("the accepted hosted revision is not an ancestor of master");
  }
  const runs = githubRunsSchema.safeParse(ghJson(`repos/${candidate.repository}/actions/runs?head_sha=${masterCommit}&status=completed&per_page=100`));
  if (!runs.success) throw new HostedCutoverFailure("GitHub returned invalid workflow runs");
  const ciMatches = runs.data.workflow_runs.filter((run) => run.name === "CI" && run.path === ".github/workflows/ci.yml" &&
    run.event === "push" && run.head_branch === "master" && run.head_sha === masterCommit && run.status === "completed" && run.conclusion === "success");
  const ci = ciMatches.length === 1 ? ciMatches[0] : undefined;
  const release = githubRunSchema.safeParse(ghJson(`repos/${candidate.repository}/actions/runs/${candidate.workflowRunId}`));
  if (ci === undefined || !release.success || release.data.name !== "Release candidates" || release.data.path !== ".github/workflows/release.yml" ||
    release.data.event !== "push" || release.data.head_branch !== candidate.tag || release.data.head_sha !== masterCommit || release.data.run_attempt !== candidate.workflowRunAttempt ||
    release.data.status !== "completed" || release.data.conclusion !== "success") {
    throw new HostedCutoverFailure("the required exact workflows are not successful");
  }
  const verdictSeed = z.object({ candidateArtifact: z.object({ id: z.string(), sha256: z.string() }), candidateManifestSha256: z.string() }).passthrough().safeParse(verdict);
  if (!verdictSeed.success) throw new HostedCutoverFailure("the acceptance verdict is invalid");
  const approval = verifyAcceptanceApprovalHistory(
    ghJson(`repos/${candidate.repository}/actions/runs/${candidate.workflowRunId}/approvals`),
    { candidate: projectAcceptanceCandidate(candidate), artifactId: verdictSeed.data.candidateArtifact.id,
      artifactSha256: verdictSeed.data.candidateArtifact.sha256, manifestSha256: verdictSeed.data.candidateManifestSha256 },
  );
  if (!approval.ok || approval.value.evidenceSha256 !== verdictSha256) throw new HostedCutoverFailure("the exact release publication approval is absent");
  const issue = githubIssueSchema.safeParse(ghJson(`repos/${candidate.repository}/issues/23`));
  const comments = githubCommentsSchema.safeParse(ghJson(`repos/${candidate.repository}/issues/23/comments?per_page=100`));
  const marker = `hosted-acceptance-v1 sha256=${hostedAcceptanceSha256} revision=${plan.replacement.revision} image=${plan.replacement.image.digest}`;
  const hostedApprovals = comments.success ? comments.data.filter((comment) => comment.author_association === "OWNER" && comment.body.trim() === marker) : [];
  if (!issue.success || hostedApprovals.length !== 1) throw new HostedCutoverFailure("issue #23 lacks the exact owner-hosted acceptance approval");
  return {
    masterCommit,
    workflows: {
      schemaVersion: 1, repository: candidate.repository, masterCommit,
      ci: projectRun(ci, "master"), release: projectRun(release.data, candidate.tag),
      hostedRevision: { baseCommit: plan.replacement.revision, headCommit: masterCommit,
        relation: compare.data.status === "identical" ? "equal" : "ancestor" },
      publicationApproval: { verified: true, evidenceSha256: approval.value.evidenceSha256 },
      hostedApproval: { issueNumber: 23, issueState: "closed", approver: hostedApprovals[0]?.user.login,
        acceptanceSha256: hostedAcceptanceSha256, revision: plan.replacement.revision, imageDigest: plan.replacement.image.digest },
    },
  };
}

function projectRun(run: z.infer<typeof githubRunSchema>, refName: string) {
  return { workflowName: run.name, workflowPath: run.path, runId: String(run.id), runAttempt: run.run_attempt,
    headSha: run.head_sha, event: run.event, refName, conclusion: run.conclusion };
}

function parseManifest(input: unknown): CandidateManifest {
  const seed = manifestSeedSchema.safeParse(input);
  if (!seed.success) throw new HostedCutoverFailure("the candidate manifest is invalid");
  const version = parseProductVersion(seed.data.version);
  if (!version.ok) throw new HostedCutoverFailure(version.reason);
  const parsed = parseCandidateManifest(input, version.value, seed.data.tag, seed.data.commit);
  if (!parsed.ok) throw new HostedCutoverFailure(parsed.reason);
  return parsed.value;
}

function parsePlanSeed(input: unknown): HostedCutoverPlan {
  const parsed = parseHostedCutoverPlan(input);
  if (!parsed.ok) throw new HostedCutoverFailure(parsed.reason);
  return parsed.value;
}

function readHostedAcceptanceReference(input: unknown): string {
  const parsed = z.object({ image: z.object({ reference: z.string() }) }).passthrough().safeParse(input);
  if (!parsed.success) throw new HostedCutoverFailure("the hosted acceptance is invalid");
  return parsed.data.image.reference;
}

async function readEvidenceKey(path: string): Promise<Buffer> {
  const key = await readReleasePrivateRegularFile(path, 64);
  if (key.byteLength !== 32 || key.every((byte) => byte === key[0]) || new Set(key).size < 8) throw new HostedCutoverFailure("the evidence key must be exactly 32 nontrivial private random bytes");
  return key;
}
async function readBootId(): Promise<Buffer> {
  const value = await readFile("/proc/sys/kernel/random/boot_id");
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\n?$/u.test(value.toString("utf8"))) {
    throw new HostedCutoverFailure("the kernel boot identity is invalid");
  }
  return value;
}
async function readBootUptimeMilliseconds(): Promise<bigint> {
  const value = await readFile("/proc/uptime", "utf8");
  const match = /^(\d+)\.(\d{1,2})\s/u.exec(value);
  if (match === null) throw new HostedCutoverFailure("the kernel boot uptime is invalid");
  return BigInt(match[1] ?? "0") * 1_000n + BigInt((match[2] ?? "0").padEnd(2, "0")) * 10n;
}

async function privateBinding(label: string, path: string, key: Buffer): Promise<string> {
  return privateBindingBytes(label, await readReleasePrivateRegularFile(path), key);
}
function privateBindingBytes(label: string, contents: Buffer, key: Buffer): string {
  return `hmac-sha256:${createHmac("sha256", key).update(label).update("\0").update(contents).digest("hex")}`;
}
function evidenceHmac(label: string, value: unknown, key: Buffer): string {
  return createHmac("sha256", key).update(label).update("\0").update(JSON.stringify(value)).digest("hex");
}
async function fileSha256(path: string): Promise<string> {
  return createHash("sha256").update(await readReleaseRegularFile(path)).digest("hex");
}
async function readJson(path: string): Promise<unknown> {
  try { return JSON.parse((await readReleaseRegularFile(path)).toString("utf8")); }
  catch { throw new HostedCutoverFailure(`${basename(path)} is not valid JSON`); }
}
async function readJsonSnapshot(path: string): Promise<{ readonly value: unknown; readonly sha256: string }> {
  const bytes = await readReleaseRegularFile(path);
  try { return { value: JSON.parse(bytes.toString("utf8")), sha256: createHash("sha256").update(bytes).digest("hex") }; }
  catch { throw new HostedCutoverFailure(`${basename(path)} is not valid JSON`); }
}
async function writePrivateJson(path: string, value: unknown): Promise<void> {
  const target = await prepareReleaseOutput(path, []);
  try { await writeReleasePrivateFile(target, `${JSON.stringify(value, null, 2)}\n`); }
  finally { await target.close(); }
}

function ghJson(endpoint: string): unknown {
  return parseJson(run("gh", ["api", "--method", "GET", "--header", "Accept: application/vnd.github+json",
    "--hostname", "github.com", "--header", "X-GitHub-Api-Version: 2026-03-10", endpoint], "read GitHub readiness"));
}
function run(command: string, arguments_: readonly string[], label: string): string {
  const environment: NodeJS.ProcessEnv = command === "compose"
    ? { PATH: "/usr/bin:/bin", HOME: process.env.HOME }
    : { ...process.env };
  if (command === "gh") { delete environment.GH_HOST; delete environment.GH_ENTERPRISE_TOKEN; }
  if (command === "docker" || command === "compose" || command === "buildx") {
    delete environment.DOCKER_HOST; delete environment.DOCKER_CONTEXT; delete environment.DOCKER_TLS_VERIFY;
    delete environment.DOCKER_CERT_PATH; delete environment.DOCKER_CONFIG; delete environment.DOCKER_CLI_PLUGIN_EXTRA_DIRS;
  }
  environment.PATH = "/usr/bin:/bin";
  const executable = command === "gh" ? "/usr/bin/gh" : command === "docker" ? "/usr/bin/docker" : command === "compose" ? trustedTool("compose") : command === "buildx" ? trustedTool("buildx") : command;
  const boundedArguments = command === "docker" && boundDockerContext !== undefined && arguments_[0] !== "context"
    ? ["--context", boundDockerContext, ...arguments_] : arguments_;
  const result = spawnSync(executable, boundedArguments, { cwd: REPOSITORY_ROOT, env: environment, encoding: "utf8", maxBuffer: MAX_COMMAND_OUTPUT, timeout: 60_000 });
  if (result.status !== 0) throw new HostedCutoverFailure(`${label} failed`);
  return result.stdout;
}
function trustedTool(name: keyof typeof TRUSTED_TOOLS): string {
  const tool = TRUSTED_TOOLS[name]; const status = statSync(tool.path);
  if (status.uid !== 0 || !status.isFile() || (status.mode & 0o022) !== 0 || createHash("sha256").update(readFileSync(tool.path)).digest("hex") !== tool.sha256) {
    throw new HostedCutoverFailure(`the pinned ${name} adapter is unavailable or altered`);
  }
  return tool.path;
}
function parseJson(input: string): unknown {
  try { return JSON.parse(input); } catch { throw new HostedCutoverFailure("a trusted command returned invalid JSON"); }
}
function parseArguments(argv: readonly string[]): CliArguments {
  const command = argv[0]; if (command === undefined) throw new HostedCutoverFailure("a hosted-cutover command is required");
  const values = new Map<string, string>();
  for (let index = 1; index < argv.length; index += 2) {
    const flag = argv[index]; const value = argv[index + 1];
    if (flag === undefined || value === undefined || !flag.startsWith("--") || values.has(flag)) throw new HostedCutoverFailure("arguments must be unique flag/value pairs");
    values.set(flag, value);
  }
  return { command, values };
}
function requireExactFlags(values: ReadonlyMap<string, string>, flags: readonly string[]): void {
  if (values.size !== flags.length || flags.some((flag) => !values.has(flag))) throw new HostedCutoverFailure(`expected exactly ${flags.join(", ")}`);
}
function required(values: ReadonlyMap<string, string>, name: string): string {
  const value = values.get(name); if (value === undefined || value.length === 0) throw new HostedCutoverFailure(`${name} is required`); return value;
}
function present(values: ReadonlyMap<string, string>, name: string): string {
  const value = values.get(name); if (value === undefined) throw new HostedCutoverFailure(`${name} is required`); return value;
}
function optional(values: ReadonlyMap<string, string>, name: string): string | undefined { return values.get(name); }
function repositoryPath(input: string): string { return isAbsolute(input) ? resolve(input) : resolve(REPOSITORY_ROOT, input); }

main().catch((error: unknown) => {
  process.stderr.write(`${error instanceof HostedCutoverFailure ? error.message : "hosted cutover failed"}\n`);
  process.exitCode = 1;
});
