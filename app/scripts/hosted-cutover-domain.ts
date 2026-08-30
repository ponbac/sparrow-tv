import { z } from "zod";
import {
  parseSealedReleaseAcceptanceVerdict,
  projectAcceptanceCandidate,
  type AcceptanceCandidateBinding,
} from "./release-acceptance-domain.ts";
import {
  parseCandidateManifest,
  parseProductVersion,
  type CandidateManifest,
  type ParseResult,
} from "./release-contract-domain.ts";

const SHA256 = /^[0-9a-f]{64}$/u;
const OCI_DIGEST = /^sha256:[0-9a-f]{64}$/u;
const FULL_COMMIT = /^[0-9a-f]{40}$/u;
const POSITIVE_IDENTIFIER = /^[1-9][0-9]*$/u;
const PRIVATE_BINDING = /^hmac-sha256:[0-9a-f]{64}$/u;
const IMMUTABLE_IMAGE =
  /^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*(?::[0-9]+)?\/)?[a-z0-9]+(?:[._-][a-z0-9]+)*(?:\/[a-z0-9]+(?:[._-][a-z0-9]+)*)*(?::[A-Za-z0-9_][A-Za-z0-9_.-]{0,127})?@sha256:[0-9a-f]{64}$/u;
const MAX_REHEARSAL_AGE_MS = 7 * 24 * 60 * 60 * 1_000;
const MAX_BASELINE_AGE_MS = 15 * 60 * 1_000;
const MAX_READINESS_AGE_MS = 30 * 60 * 1_000;
const MAX_CUTOVER_MS = 60 * 60 * 1_000;
const MAX_SEAL_DELAY_MS = 5 * 60 * 1_000;

/** Complete replacement endpoint gates; response bodies and catalog values are never recorded. */
export const HOSTED_ENDPOINT_GATES = [
  "health",
  "ui-authentication",
  "api-authentication",
  "refresh-state",
  "browse",
  "search",
  "guide",
  "playback-by-channel-id",
  "privacy-projection",
] as const;

/** Liveness gates supported by the pre-rewrite 0.11.4 HTTP contract. */
export const LEGACY_ROLLBACK_GATES = [
  "legacy-ui-liveness",
  "legacy-search-liveness",
] as const;

/** Immutable 0.11.4 manifest already identified by the hosted checkpoint. */
export const LEGACY_ROLLBACK_DIGEST =
  "sha256:96ac1b8e3fe6f25bc912a62a4b457be4fd553bc9a6e72db6fc6dffde2e8ff30f";

/** SHA-256 of the committed synthetic hosted-rehearsal fixture bytes. */
export const HOSTED_REHEARSAL_FIXTURE_SHA256 =
  "d75c9cd4e8e0928904e191464f5ab73ad3c1d5fdad8b7c55cc19bc575e852991";

const sha256Schema = z.string().regex(SHA256);
const digestSchema = z.string().regex(OCI_DIGEST);
const commitSchema = z.string().regex(FULL_COMMIT);
const imageReferenceSchema = z.string().regex(IMMUTABLE_IMAGE).max(512);
const privateBindingSchema = z.string().regex(PRIVATE_BINDING);
const timestampSchema = z.string().datetime({ offset: true });
const dockerContextClassSchema = z.enum(["local-unix", "remote-ssh", "remote-tls"]);

const deploymentContractSchema = z
  .object({
    schemaVersion: z.literal(1),
    repository: z.literal("ponbac/sparrow-tv"),
    service: z.object({ name: z.literal("sparrow"), containerPort: z.literal(33733) }).strict(),
    reverseProxy: z
      .object({
        publicOrigin: z.literal("https://tv.ponbac.xyz"),
        upstream: z.literal("sparrow:33733"),
        mutation: z.literal("forbidden"),
      })
      .strict(),
    rollback: z
      .object({
        baselineTag: z.literal("docker.io/ponbac/sparrow:0.11.4"),
        immutableDigest: z.literal(LEGACY_ROLLBACK_DIGEST),
        digestSource: z.literal("hosted-checkpoint"),
      })
      .strict(),
    rehearsalFixture: z.object({
      image: z.literal("docker.io/library/python:3.13.15-alpine3.24@sha256:540c7d91f98ff6880174c40e99067bf5941eb54d818a7a5e094d188b196a934d"),
      script: z.literal("scripts/hosted-rehearsal-fixture.py"),
      scriptSha256: z.literal(HOSTED_REHEARSAL_FIXTURE_SHA256),
    }).strict(),
  })
  .strict();

const topologySchema = z
  .object({
    serviceName: z.literal("sparrow"),
    containerPort: z.literal(33733),
    publicOrigin: z.literal("https://tv.ponbac.xyz"),
    caddyUpstream: z.literal("sparrow:33733"),
    caddyMutation: z.literal("forbidden"),
    dockerContextClass: dockerContextClassSchema,
    dockerEndpointBinding: privateBindingSchema,
    runtimeTopologyBinding: privateBindingSchema,
    baselineLifecycleBinding: privateBindingSchema,
  })
  .strict();
const imageBindingSchema = z.object({ reference: imageReferenceSchema, digest: digestSchema }).strict();
const privateConfigurationSchema = z
  .object({
    baselineComposeBinding: privateBindingSchema,
    candidateComposeBinding: privateBindingSchema,
    baselineServiceBinding: privateBindingSchema,
    candidateServiceBinding: privateBindingSchema,
    environmentBinding: privateBindingSchema,
    caddyBinding: privateBindingSchema,
  })
  .strict();
const endpointGateSchema = z.object({ id: z.string().min(1).max(96), result: z.literal("passed") }).strict();
const replacementEndpointSchema = z
  .object({
    schemaVersion: z.literal(1), recordedAt: timestampSchema,
    targetOrigin: z.union([z.literal("https://tv.ponbac.xyz"), z.string().regex(/^http:\/\/(127\.0\.0\.1|localhost):([1-9][0-9]{0,4})$/u)]),
    role: z.enum(["candidate", "production"]), result: z.literal("passed"),
    gates: z.array(endpointGateSchema).max(HOSTED_ENDPOINT_GATES.length),
  }).strict();
const legacyEndpointSchema = z
  .object({
    schemaVersion: z.literal(1), recordedAt: timestampSchema,
    targetOrigin: z.union([z.literal("https://tv.ponbac.xyz"), z.string().regex(/^http:\/\/(127\.0\.0\.1|localhost):([1-9][0-9]{0,4})$/u)]),
    role: z.enum(["baseline", "rollback", "baseline-production", "rollback-production"]), result: z.literal("passed"),
    gates: z.array(endpointGateSchema).max(LEGACY_ROLLBACK_GATES.length),
  }).strict();
const hostedAcceptanceSchema = z
  .object({
    schemaVersion: z.literal(1), verdict: z.literal("hosted-accepted"), recordedAt: timestampSchema,
    image: imageBindingSchema, revision: commitSchema, reproducedManifestDigest: digestSchema,
    containerPort: z.literal(33733), endpoint: replacementEndpointSchema,
    fixture: z.object({ image: imageBindingSchema, scriptSha256: sha256Schema }).strict(),
  }).strict();
const trustedPreparationSchema = z
  .object({
    configuration: privateConfigurationSchema,
    replacementImage: imageBindingSchema,
    baseline: z.object({
      image: imageBindingSchema, containerIdentitySha256: sha256Schema,
      serviceName: z.literal("sparrow"), containerPort: z.literal(33733),
      dockerContextClass: dockerContextClassSchema,
      dockerEndpointBinding: privateBindingSchema,
      runtimeTopologyBinding: privateBindingSchema,
      lifecycleBinding: privateBindingSchema,
    }).strict(),
    backupsReadable: z.literal(true), replacementRegistryAvailable: z.literal(true), rollbackRegistryAvailable: z.literal(true),
  }).strict();
const hostedCutoverPlanSchema = z
  .object({
    schemaVersion: z.literal(1), plan: z.literal("hosted-hard-cutover"), preparedAt: timestampSchema,
    repository: z.literal("ponbac/sparrow-tv"), contractSha256: sha256Schema, hostedAcceptanceSha256: sha256Schema,
    topology: topologySchema,
    replacement: z.object({ image: imageBindingSchema, revision: commitSchema, reproducedManifestDigest: digestSchema }).strict(),
    rollback: z.object({ baselineTag: z.literal("docker.io/ponbac/sparrow:0.11.4"), image: imageBindingSchema }).strict(),
    configuration: privateConfigurationSchema,
    observedBaseline: z.object({ image: imageBindingSchema, containerIdentitySha256: sha256Schema }).strict(),
    registry: z.object({ replacementAvailable: z.literal(true), rollbackAvailable: z.literal(true) }).strict(),
    backupsReadable: z.literal(true),
    fixture: z.object({ image: imageBindingSchema, scriptSha256: sha256Schema }).strict(),
  }).strict();

const baselineStepSchema = z.object({
  role: z.literal("baseline"), image: imageBindingSchema, revision: commitSchema.nullable(),
  serviceName: z.literal("sparrow"), containerPort: z.literal(33733), endpoint: legacyEndpointSchema,
}).strict();
const candidateStepSchema = z.object({
  role: z.literal("candidate"), image: imageBindingSchema, revision: commitSchema,
  serviceName: z.literal("sparrow"), containerPort: z.literal(33733), endpoint: replacementEndpointSchema,
}).strict();
const rollbackStepSchema = z.object({
  role: z.literal("rollback"), image: imageBindingSchema, revision: commitSchema.nullable(),
  serviceName: z.literal("sparrow"), containerPort: z.literal(33733), endpoint: legacyEndpointSchema,
}).strict();
const rehearsalObservationSchema = z.object({
  schemaVersion: z.literal(1), rehearsal: z.literal("isolated-baseline-candidate-rollback"), recordedAt: timestampSchema,
  environmentBinding: privateBindingSchema, dockerContextClass: z.literal("local-unix"),
  steps: z.tuple([baselineStepSchema, candidateStepSchema, rollbackStepSchema]),
  fixture: z.object({ image: imageBindingSchema, scriptSha256: sha256Schema }).strict(),
}).strict();
const verifiedRehearsalSchema = z.object({
  schemaVersion: z.literal(1), verdict: z.literal("rollback-rehearsed"), recordedAt: timestampSchema, verifiedAt: timestampSchema,
  replacementRevision: commitSchema, replacementImageDigest: digestSchema, rollbackImageDigest: digestSchema,
  environmentBinding: privateBindingSchema, steps: z.tuple([baselineStepSchema, candidateStepSchema, rollbackStepSchema]),
  fixture: z.object({ image: imageBindingSchema, scriptSha256: sha256Schema }).strict(),
}).strict();

const workflowRunSchema = z.object({
  workflowName: z.string(), workflowPath: z.string(), runId: z.string().regex(POSITIVE_IDENTIFIER),
  runAttempt: z.number().int().positive(), headSha: commitSchema, event: z.literal("push"),
  refName: z.string().min(1).max(255), conclusion: z.literal("success"),
}).strict();
const workflowEvidenceSchema = z.object({
  schemaVersion: z.literal(1), repository: z.literal("ponbac/sparrow-tv"), masterCommit: commitSchema,
  ci: workflowRunSchema, release: workflowRunSchema,
  hostedRevision: z.object({ baseCommit: commitSchema, headCommit: commitSchema, relation: z.enum(["ancestor", "equal"]) }).strict(),
  publicationApproval: z.object({ verified: z.literal(true), evidenceSha256: sha256Schema }).strict(),
  hostedApproval: z.object({ issueNumber: z.literal(23), issueState: z.literal("closed"), approver: z.string().min(1),
    acceptanceSha256: sha256Schema, revision: commitSchema, imageDigest: digestSchema }).strict(),
}).strict();
const baselineObservationSchema = z.object({
  schemaVersion: z.literal(1), recordedAt: timestampSchema, image: imageBindingSchema,
  containerIdentitySha256: sha256Schema, topology: topologySchema, configuration: privateConfigurationSchema,
  backupsReadable: z.literal(true), replacementRegistryAvailable: z.literal(true), rollbackRegistryAvailable: z.literal(true),
}).strict();
const readinessSchema = z.object({
  schemaVersion: z.literal(1), verdict: z.literal("preconditions-satisfied"), verifiedAt: timestampSchema,
  repository: z.literal("ponbac/sparrow-tv"), masterCommit: commitSchema, topology: topologySchema,
  baselineContainerIdentitySha256: sha256Schema,
  replacement: z.object({ image: imageBindingSchema, revision: commitSchema }).strict(),
  rollback: z.object({ baselineTag: z.literal("docker.io/ponbac/sparrow:0.11.4"), image: imageBindingSchema }).strict(),
  configuration: privateConfigurationSchema,
  bindings: z.object({ planSha256: sha256Schema, rehearsalSha256: sha256Schema, candidateManifestSha256: sha256Schema, acceptanceVerdictSha256: sha256Schema }).strict(),
  hostedAcceptance: z.object({ fileSha256: sha256Schema, revisionRelation: z.enum(["ancestor", "equal"]) }).strict(),
  releaseAcceptance: z.object({
    version: z.string(), tag: z.string(), workflowRunId: z.string().regex(POSITIVE_IDENTIFIER),
    workflowRunAttempt: z.number().int().positive(), artifactId: z.string().regex(POSITIVE_IDENTIFIER), artifactSha256: sha256Schema,
  }).strict(),
  workflows: z.object({ ci: workflowRunSchema, release: workflowRunSchema }).strict(),
}).strict();

const runtimeSchema = z.object({
  image: imageBindingSchema, revision: commitSchema.nullable(), serviceName: z.literal("sparrow"), containerPort: z.literal(33733),
  dockerContextClass: dockerContextClassSchema, containerIdentitySha256: sha256Schema,
  dockerEndpointBinding: privateBindingSchema,
  runtimeTopologyBinding: privateBindingSchema,
  lifecycleBinding: privateBindingSchema,
}).strict();
const runtimeConfigurationSchema = z.object({
  composeBinding: privateBindingSchema, environmentBinding: privateBindingSchema, caddyBinding: privateBindingSchema,
}).strict();
const rollbackProofSchema = z.object({ performed: z.literal(true), runtime: runtimeSchema, configuration: runtimeConfigurationSchema, endpoint: legacyEndpointSchema }).strict();
const retainedBaselineSchema = z.object({
  performed: z.literal(false), reason: z.literal("baseline-unmodified"), runtime: runtimeSchema,
  configuration: runtimeConfigurationSchema, endpoint: legacyEndpointSchema,
}).strict();
const deployedCandidateSchema = z.object({ runtime: runtimeSchema, configuration: runtimeConfigurationSchema }).strict();
const productionObservationSchema = z.object({
  schemaVersion: z.literal(1), readinessSha256: sha256Schema, startedAt: timestampSchema, completedAt: timestampSchema,
  outage: z.object({ startedAt: timestampSchema.nullable(), restoredAt: timestampSchema.nullable() }).strict(),
  attempt: z.discriminatedUnion("result", [
    z.object({ result: z.literal("passed"), deployed: deployedCandidateSchema, endpoint: replacementEndpointSchema }).strict(),
    z.object({
      result: z.literal("registry-failed"), failure: z.literal("registry-pull-failed"),
      deployed: z.null(), rollback: retainedBaselineSchema,
    }).strict(),
    z.object({
      result: z.literal("failed"), failure: z.literal("service-recreate-failed"),
      deployed: z.null(), rollback: rollbackProofSchema,
    }).strict(),
    z.object({
      result: z.literal("failed-after-start"),
      failure: z.enum(["healthcheck-failed", "topology-mismatch", "runtime-crash", "hosted-endpoint-verification-failed"]),
      rollback: rollbackProofSchema,
    }).strict(),
    z.object({
      result: z.literal("recovery-failed"),
      failure: z.enum(["service-recreate-failed", "healthcheck-failed", "topology-mismatch", "runtime-crash", "hosted-endpoint-verification-failed"]),
      incident: z.object({ opened: z.literal(true), reference: z.string().regex(/^INC-[A-Z0-9-]{4,64}$/u) }).strict(),
    }).strict(),
  ]),
}).strict();
// The schema is the single source for the exported sealed-evidence type.
// eslint-disable-next-line @typescript-eslint/no-unused-vars
const productionEvidenceSchema = z.object({
  schemaVersion: z.literal(1), verdict: z.enum(["production-accepted", "production-baseline-retained", "production-rolled-back", "production-recovery-failed"]), sealedAt: timestampSchema,
  readinessSha256: sha256Schema, masterCommit: commitSchema, replacementImageDigest: digestSchema,
  runtimeImageDigest: digestSchema.nullable(), rollbackImageDigest: digestSchema, downtimeSeconds: z.number().int().nonnegative().max(3600).nullable(),
  observedOutageSeconds: z.number().int().nonnegative().max(3600),
  configuration: runtimeConfigurationSchema.nullable(),
  verification: z.discriminatedUnion("result", [
    z.object({ result: z.literal("passed"), endpoint: replacementEndpointSchema }).strict(),
    z.object({ result: z.literal("failed-then-rolled-back"), endpoint: legacyEndpointSchema }).strict(),
    z.object({ result: z.literal("baseline-retained"), endpoint: legacyEndpointSchema }).strict(),
    z.object({ result: z.literal("incident-open"), incidentReference: z.string().regex(/^INC-[A-Z0-9-]{4,64}$/u) }).strict(),
  ]),
}).strict();
const manifestSeedSchema = z.object({ version: z.string(), tag: z.string(), commit: z.string() }).passthrough();

/** Parsed immutable image identity. */
export type HostedImageBinding = z.infer<typeof imageBindingSchema>;
/** Safe preparatory facts; this record grants no mutation authority. */
export type HostedCutoverPlan = z.infer<typeof hostedCutoverPlanSchema>;
/** Privacy-safe isolated rollback proof. */
export type VerifiedHostedRehearsal = z.infer<typeof verifiedRehearsalSchema>;
/** Exact preconditions for later explicit owner authorization. */
export type HostedCutoverReadiness = z.infer<typeof readinessSchema>;
/** Safe accepted-production or restored-rollback projection. */
export type HostedProductionEvidence = z.infer<typeof productionEvidenceSchema>;

/** Parses the committed topology and immutable rollback policy. */
export function parseHostedDeploymentContract(input: unknown): ParseResult<z.infer<typeof deploymentContractSchema>> {
  const parsed = deploymentContractSchema.safeParse(input);
  return parsed.success ? accept(parsed.data) : reject("the hosted deployment contract is invalid or has drifted");
}

/** Parses an OCI digest reference and projects the manifest digest. */
export function parseHostedImageReference(input: unknown): ParseResult<HostedImageBinding> {
  const reference = imageReferenceSchema.safeParse(input);
  if (!reference.success) return reject("the hosted image must be an immutable OCI digest reference");
  const offset = reference.data.lastIndexOf("@sha256:");
  const digest = digestSchema.safeParse(offset < 1 ? "" : reference.data.slice(offset + 1));
  return digest.success ? accept({ reference: reference.data, digest: digest.data }) : reject("the hosted image must be an immutable OCI digest reference");
}

/** Parses a previously prepared hosted cutover plan without stripping unknown fields. */
export function parseHostedCutoverPlan(input: unknown): ParseResult<HostedCutoverPlan> {
  const parsed = hostedCutoverPlanSchema.safeParse(input);
  return parsed.success ? accept(parsed.data) : reject("the hosted cutover plan is invalid");
}

/** Parses exact readiness evidence before a production observation is enriched. */
export function parseHostedCutoverReadiness(input: unknown): ParseResult<HostedCutoverReadiness> {
  const parsed = readinessSchema.safeParse(input);
  return parsed.success ? accept(parsed.data) : reject("the hosted cutover readiness record is invalid");
}

/** Parses one complete privacy-safe endpoint record for its declared role. */
export function parseHostedEndpointEvidence(input: unknown): ParseResult<unknown> {
  const replacement = replacementEndpointSchema.safeParse(input);
  if (replacement.success && completeReplacementEndpoint(replacement.data, replacement.data.role)) {
    return accept(replacement.data);
  }
  const legacy = legacyEndpointSchema.safeParse(input);
  if (legacy.success && completeLegacyEndpoint(legacy.data, legacy.data.role)) return accept(legacy.data);
  return reject("the hosted endpoint evidence is incomplete or invalid");
}

/** Parses the exact structured hosted checkpoint consumed by cutover preparation. */
export function parseHostedAcceptanceEvidence(input: unknown): ParseResult<unknown> {
  const parsed = hostedAcceptanceSchema.safeParse(input);
  return parsed.success && completeReplacementEndpoint(parsed.data.endpoint, "candidate")
    ? accept(parsed.data)
    : reject("the structured hosted acceptance is invalid");
}

/** Creates a fail-closed plan from adapter-recomputed private and runtime facts. */
export function prepareHostedCutover(input: {
  readonly contract: unknown; readonly contractSha256: unknown; readonly hostedAcceptance: unknown;
  readonly hostedAcceptanceSha256: unknown; readonly preparedAt: unknown; readonly trustedFacts: unknown;
}): ParseResult<HostedCutoverPlan> {
  const contract = parseHostedDeploymentContract(input.contract);
  const contractSha256 = sha256Schema.safeParse(input.contractSha256);
  const acceptance = hostedAcceptanceSchema.safeParse(input.hostedAcceptance);
  const acceptanceSha256 = sha256Schema.safeParse(input.hostedAcceptanceSha256);
  const preparedAt = timestampSchema.safeParse(input.preparedAt);
  const facts = trustedPreparationSchema.safeParse(input.trustedFacts);
  if (!contract.ok || !contractSha256.success || !acceptance.success || !acceptanceSha256.success || !preparedAt.success || !facts.success) {
    return reject("the hosted cutover preparation input is invalid");
  }
  const replacement = parseHostedImageReference(acceptance.data.image.reference);
  const rollbackReference = `${contract.value.rollback.baselineTag}@${contract.value.rollback.immutableDigest}`;
  const rollback = parseHostedImageReference(rollbackReference);
  const fixture = parseHostedImageReference(contract.value.rehearsalFixture.image);
  if (!replacement.ok || !rollback.ok || !fixture.ok || !sameImage(replacement.value, acceptance.data.image) ||
    !sameImage(fixture.value, acceptance.data.fixture.image) ||
    acceptance.data.fixture.scriptSha256 !== contract.value.rehearsalFixture.scriptSha256 ||
    replacement.value.digest !== acceptance.data.reproducedManifestDigest ||
    facts.data.replacementImage.digest !== replacement.value.digest ||
    !completeReplacementEndpoint(acceptance.data.endpoint, "candidate") || !sameImage(facts.data.baseline.image, rollback.value)) {
    return reject("the hosted acceptance or immutable rollback identity is inconsistent");
  }
  if (Date.parse(acceptance.data.recordedAt) > Date.parse(preparedAt.data)) return reject("the hosted acceptance was recorded after cutover preparation");
  return accept({
    schemaVersion: 1, plan: "hosted-hard-cutover", preparedAt: preparedAt.data, repository: contract.value.repository,
    contractSha256: contractSha256.data, hostedAcceptanceSha256: acceptanceSha256.data,
    topology: {
      serviceName: contract.value.service.name, containerPort: contract.value.service.containerPort,
      publicOrigin: contract.value.reverseProxy.publicOrigin, caddyUpstream: contract.value.reverseProxy.upstream,
      caddyMutation: contract.value.reverseProxy.mutation, dockerContextClass: facts.data.baseline.dockerContextClass,
      dockerEndpointBinding: facts.data.baseline.dockerEndpointBinding,
      runtimeTopologyBinding: facts.data.baseline.runtimeTopologyBinding,
      baselineLifecycleBinding: facts.data.baseline.lifecycleBinding,
    },
    replacement: { image: facts.data.replacementImage, revision: acceptance.data.revision, reproducedManifestDigest: acceptance.data.reproducedManifestDigest },
    rollback: { baselineTag: contract.value.rollback.baselineTag, image: rollback.value },
    configuration: facts.data.configuration,
    observedBaseline: { image: facts.data.baseline.image, containerIdentitySha256: facts.data.baseline.containerIdentitySha256 },
    registry: { replacementAvailable: true, rollbackAvailable: true }, backupsReadable: true,
    fixture: acceptance.data.fixture,
  });
}

/** Requires the exact isolated legacy baseline → replacement → legacy rollback sequence. */
export function verifyHostedRehearsal(input: { readonly plan: unknown; readonly observation: unknown; readonly verifiedAt: unknown }): ParseResult<VerifiedHostedRehearsal> {
  const plan = hostedCutoverPlanSchema.safeParse(input.plan);
  const observation = rehearsalObservationSchema.safeParse(input.observation);
  const verifiedAt = timestampSchema.safeParse(input.verifiedAt);
  if (!plan.success || !observation.success || !verifiedAt.success) return reject("the hosted rollback rehearsal input is invalid");
  const [baseline, candidate, rollback] = observation.data.steps;
  if (!sameImage(baseline.image, plan.data.rollback.image) || !sameImage(candidate.image, plan.data.replacement.image) ||
    !sameImage(rollback.image, plan.data.rollback.image) || candidate.revision !== plan.data.replacement.revision ||
    !sameImage(observation.data.fixture.image, plan.data.fixture.image) || observation.data.fixture.scriptSha256 !== plan.data.fixture.scriptSha256 ||
    !completeLegacyEndpoint(baseline.endpoint, "baseline") || !completeReplacementEndpoint(candidate.endpoint, "candidate") ||
    !completeLegacyEndpoint(rollback.endpoint, "rollback")) {
    return reject("the isolated rehearsal did not prove the exact replacement and rollback path");
  }
  if (![baseline.endpoint.recordedAt, candidate.endpoint.recordedAt, rollback.endpoint.recordedAt]
    .every((value) => within(value, plan.data.preparedAt, observation.data.recordedAt))) {
    return reject("the rehearsal endpoint timestamps are outside the rehearsal window");
  }
  if (!ordered([plan.data.preparedAt, observation.data.recordedAt, verifiedAt.data])) return reject("the rollback rehearsal timestamps are out of order");
  return accept({
    schemaVersion: 1, verdict: "rollback-rehearsed", recordedAt: observation.data.recordedAt, verifiedAt: verifiedAt.data,
    replacementRevision: plan.data.replacement.revision, replacementImageDigest: plan.data.replacement.image.digest,
    rollbackImageDigest: plan.data.rollback.image.digest, environmentBinding: observation.data.environmentBinding,
    fixture: observation.data.fixture,
    steps: observation.data.steps,
  });
}

/** Verifies merge, ancestor-hosted acceptance, publication approval, CI, backups, registry, and rollback proof. */
export function verifyHostedCutoverReadiness(input: {
  readonly plan: unknown; readonly planSha256: unknown; readonly rehearsal: unknown; readonly rehearsalSha256: unknown;
  readonly hostedAcceptance: unknown; readonly hostedAcceptanceSha256: unknown;
  readonly candidateManifest: unknown; readonly candidateManifestSha256: unknown; readonly acceptanceVerdict: unknown;
  readonly acceptanceVerdictSha256: unknown; readonly workflowEvidence: unknown; readonly baselineObservation: unknown;
  readonly masterCommit: unknown; readonly verifiedAt: unknown;
}): ParseResult<HostedCutoverReadiness> {
  const plan = hostedCutoverPlanSchema.safeParse(input.plan);
  const rehearsal = verifiedRehearsalSchema.safeParse(input.rehearsal);
  const hostedAcceptance = hostedAcceptanceSchema.safeParse(input.hostedAcceptance);
  const hostedAcceptanceSha256 = sha256Schema.safeParse(input.hostedAcceptanceSha256);
  const verdict = parseSealedReleaseAcceptanceVerdict(input.acceptanceVerdict);
  const workflows = workflowEvidenceSchema.safeParse(input.workflowEvidence);
  const baseline = baselineObservationSchema.safeParse(input.baselineObservation);
  const master = commitSchema.safeParse(input.masterCommit);
  const verifiedAt = timestampSchema.safeParse(input.verifiedAt);
  const bindings = z.object({ planSha256: sha256Schema, rehearsalSha256: sha256Schema, candidateManifestSha256: sha256Schema, acceptanceVerdictSha256: sha256Schema }).strict().safeParse({
    planSha256: input.planSha256, rehearsalSha256: input.rehearsalSha256,
    candidateManifestSha256: input.candidateManifestSha256, acceptanceVerdictSha256: input.acceptanceVerdictSha256,
  });
  const manifest = parseManifest(input.candidateManifest);
  if (!plan.success || !rehearsal.success || !hostedAcceptance.success || !hostedAcceptanceSha256.success || !verdict.ok || !workflows.success || !baseline.success || !master.success || !verifiedAt.success || !bindings.success || !manifest.ok) {
    return reject("the hosted cutover readiness evidence is invalid");
  }
  if (hostedAcceptanceSha256.data !== plan.data.hostedAcceptanceSha256 || hostedAcceptance.data.revision !== plan.data.replacement.revision ||
    hostedAcceptance.data.image.digest !== plan.data.replacement.image.digest) return reject("the hosted acceptance bytes do not match the deployment plan");
  if (workflows.data.hostedApproval.acceptanceSha256 !== hostedAcceptanceSha256.data ||
    workflows.data.hostedApproval.revision !== hostedAcceptance.data.revision || workflows.data.hostedApproval.imageDigest !== hostedAcceptance.data.image.digest) {
    return reject("the hosted acceptance lacks the exact independent #23 approval");
  }
  if (!sameCandidate(verdict.value.candidate, projectAcceptanceCandidate(manifest.value))) return reject("the release acceptance verdict belongs to another candidate attempt");
  if (verdict.value.candidateManifestSha256 !== bindings.data.candidateManifestSha256 || verdict.value.candidate.commit !== master.data ||
    manifest.value.commit !== master.data || workflows.data.masterCommit !== master.data || plan.data.repository !== manifest.value.repository) {
    return reject("the final accepted release is not the current merged master commit");
  }
  if (workflows.data.hostedRevision.baseCommit !== plan.data.replacement.revision || workflows.data.hostedRevision.headCommit !== master.data) {
    return reject("the accepted hosted image is not an ancestor of the final replacement");
  }
  if ((workflows.data.hostedRevision.relation === "equal") !== (plan.data.replacement.revision === master.data)) {
    return reject("the hosted revision ancestry relation contradicts its exact commits");
  }
  if (rehearsal.data.replacementRevision !== plan.data.replacement.revision || rehearsal.data.replacementImageDigest !== plan.data.replacement.image.digest ||
    rehearsal.data.rollbackImageDigest !== plan.data.rollback.image.digest) {
    return reject("the rollback rehearsal belongs to another deployment plan");
  }
  if (!sameImage(baseline.data.image, plan.data.rollback.image) || baseline.data.containerIdentitySha256 !== plan.data.observedBaseline.containerIdentitySha256 ||
    !sameTopology(baseline.data.topology, plan.data.topology) || !samePrivateConfiguration(baseline.data.configuration, plan.data.configuration)) {
    return reject("the production baseline, private configuration, or Caddy route drifted");
  }
  if (!validCi(workflows.data.ci, master.data) || !validRelease(workflows.data.release, manifest.value)) return reject("the merged commit does not have the required successful workflow results");
  const now = Date.parse(verifiedAt.data);
  if (!ordered([plan.data.preparedAt, rehearsal.data.recordedAt, rehearsal.data.verifiedAt, baseline.data.recordedAt, verifiedAt.data]) ||
    now - Date.parse(rehearsal.data.verifiedAt) > MAX_REHEARSAL_AGE_MS || now - Date.parse(baseline.data.recordedAt) > MAX_BASELINE_AGE_MS ||
    workflows.data.publicationApproval.evidenceSha256 !== bindings.data.acceptanceVerdictSha256) {
    return reject("the readiness evidence is stale, out of order, or lacks exact publication approval");
  }
  return accept({
    schemaVersion: 1, verdict: "preconditions-satisfied", verifiedAt: verifiedAt.data, repository: plan.data.repository,
    masterCommit: master.data, topology: plan.data.topology,
    baselineContainerIdentitySha256: baseline.data.containerIdentitySha256,
    replacement: { image: plan.data.replacement.image, revision: plan.data.replacement.revision },
    rollback: plan.data.rollback, configuration: plan.data.configuration, bindings: bindings.data,
    hostedAcceptance: { fileSha256: plan.data.hostedAcceptanceSha256, revisionRelation: workflows.data.hostedRevision.relation },
    releaseAcceptance: {
      version: manifest.value.version, tag: manifest.value.tag, workflowRunId: manifest.value.workflowRunId,
      workflowRunAttempt: manifest.value.workflowRunAttempt, artifactId: verdict.value.candidateArtifact.id,
      artifactSha256: verdict.value.candidateArtifact.sha256,
    }, workflows: { ci: workflows.data.ci, release: workflows.data.release },
  });
}

/** Seals accepted production or any supported failed phase followed by the exact verified rollback. */
export function sealHostedProductionEvidence(input: { readonly readiness: unknown; readonly readinessSha256: unknown; readonly observation: unknown; readonly sealedAt: unknown }): ParseResult<HostedProductionEvidence> {
  const readiness = readinessSchema.safeParse(input.readiness);
  const readinessSha256 = sha256Schema.safeParse(input.readinessSha256);
  const observation = productionObservationSchema.safeParse(input.observation);
  const sealedAt = timestampSchema.safeParse(input.sealedAt);
  if (!readiness.success || !readinessSha256.success || !observation.success || !sealedAt.success) return reject("the production cutover evidence is invalid");
  const started = Date.parse(observation.data.startedAt); const completed = Date.parse(observation.data.completedAt);
  const ready = Date.parse(readiness.data.verifiedAt); const sealed = Date.parse(sealedAt.data);
  if (observation.data.readinessSha256 !== readinessSha256.data || started < ready || started - ready > MAX_READINESS_AGE_MS ||
    completed < started || completed - started > MAX_CUTOVER_MS || sealed < completed || sealed - completed > MAX_SEAL_DELAY_MS) {
    return reject("the production cutover timestamps or readiness binding are invalid");
  }
  const outageStarted = observation.data.outage.startedAt;
  const outageRestored = observation.data.outage.restoredAt;
  const incidentOpen = observation.data.attempt.result === "recovery-failed";
  if ((!incidentOpen && (outageStarted === null) !== (outageRestored === null)) ||
    (incidentOpen && (outageStarted === null || outageRestored !== null)) ||
    (outageStarted !== null && outageRestored !== null &&
      (!within(outageStarted, observation.data.startedAt, observation.data.completedAt) ||
       !within(outageRestored, outageStarted, observation.data.completedAt)))) {
    return reject("the production outage interval is invalid");
  }
  const downtimeSeconds = outageStarted === null || outageRestored === null
    ? 0
    : Math.ceil((Date.parse(outageRestored) - Date.parse(outageStarted)) / 1_000);
  const observedOutageSeconds = outageStarted === null ? 0
    : Math.ceil((Date.parse(outageRestored ?? observation.data.completedAt) - Date.parse(outageStarted)) / 1_000);
  if (observation.data.attempt.result === "passed") {
    if (!sameCandidateDeployment(observation.data.attempt.deployed, readiness.data) || !completeReplacementEndpoint(observation.data.attempt.endpoint, "production") ||
      observation.data.attempt.endpoint.targetOrigin !== readiness.data.topology.publicOrigin ||
      !within(observation.data.attempt.endpoint.recordedAt, observation.data.startedAt, observation.data.completedAt)) {
      return reject("the deployed runtime does not match the accepted hosted replacement");
    }
    return accept({
      schemaVersion: 1, verdict: "production-accepted", sealedAt: sealedAt.data, readinessSha256: readinessSha256.data,
      masterCommit: readiness.data.masterCommit, replacementImageDigest: readiness.data.replacement.image.digest,
      runtimeImageDigest: observation.data.attempt.deployed.runtime.image.digest, rollbackImageDigest: readiness.data.rollback.image.digest,
      downtimeSeconds, observedOutageSeconds, configuration: observation.data.attempt.deployed.configuration,
      verification: { result: "passed", endpoint: observation.data.attempt.endpoint },
    });
  }
  if (observation.data.attempt.result === "recovery-failed") {
    return accept({
      schemaVersion: 1, verdict: "production-recovery-failed", sealedAt: sealedAt.data,
      readinessSha256: readinessSha256.data, masterCommit: readiness.data.masterCommit,
      replacementImageDigest: readiness.data.replacement.image.digest, runtimeImageDigest: null,
      rollbackImageDigest: readiness.data.rollback.image.digest, downtimeSeconds: null, observedOutageSeconds,
      configuration: null,
      verification: { result: "incident-open", incidentReference: observation.data.attempt.incident.reference },
    });
  }
  if (observation.data.attempt.result === "registry-failed") {
    if (outageStarted !== null || !sameRetainedBaseline(observation.data.attempt.rollback, readiness.data) ||
      !within(observation.data.attempt.rollback.endpoint.recordedAt, observation.data.startedAt, observation.data.completedAt)) {
      return reject("the registry failure did not prove that the baseline remained available");
    }
    return accept({
      schemaVersion: 1, verdict: "production-baseline-retained", sealedAt: sealedAt.data,
      readinessSha256: readinessSha256.data, masterCommit: readiness.data.masterCommit,
      replacementImageDigest: readiness.data.replacement.image.digest,
      runtimeImageDigest: observation.data.attempt.rollback.runtime.image.digest,
      rollbackImageDigest: readiness.data.rollback.image.digest, downtimeSeconds, observedOutageSeconds,
      configuration: observation.data.attempt.rollback.configuration,
      verification: { result: "baseline-retained", endpoint: observation.data.attempt.rollback.endpoint },
    });
  }
  const rollback = observation.data.attempt.rollback;
  if (!sameRollback(rollback, readiness.data) || !within(rollback.endpoint.recordedAt, observation.data.startedAt, observation.data.completedAt)) {
    return reject("the failed cutover was not followed by the exact verified rollback");
  }
  return accept({
    schemaVersion: 1, verdict: "production-rolled-back", sealedAt: sealedAt.data, readinessSha256: readinessSha256.data,
    masterCommit: readiness.data.masterCommit, replacementImageDigest: readiness.data.replacement.image.digest,
    runtimeImageDigest: rollback.runtime.image.digest, rollbackImageDigest: readiness.data.rollback.image.digest,
    downtimeSeconds, observedOutageSeconds, configuration: rollback.configuration,
    verification: { result: "failed-then-rolled-back", endpoint: rollback.endpoint },
  });
}

function parseManifest(input: unknown): ParseResult<CandidateManifest> {
  const seed = manifestSeedSchema.safeParse(input);
  if (!seed.success) return reject("the release candidate manifest is invalid");
  const version = parseProductVersion(seed.data.version);
  if (!version.ok) return version;
  return parseCandidateManifest(input, version.value, seed.data.tag, seed.data.commit);
}
function validCi(run: z.infer<typeof workflowRunSchema>, commit: string): boolean {
  return run.workflowName === "CI" && run.workflowPath === ".github/workflows/ci.yml" && run.headSha === commit && run.refName === "master";
}
function validRelease(run: z.infer<typeof workflowRunSchema>, manifest: CandidateManifest): boolean {
  return run.workflowName === "Release candidates" && run.workflowPath === ".github/workflows/release.yml" && run.runId === manifest.workflowRunId &&
    run.runAttempt === manifest.workflowRunAttempt && run.headSha === manifest.commit && run.refName === manifest.tag;
}
function sameCandidateDeployment(deployed: z.infer<typeof deployedCandidateSchema>, ready: HostedCutoverReadiness): boolean {
  return sameImage(deployed.runtime.image, ready.replacement.image) && deployed.runtime.revision === ready.replacement.revision &&
    deployed.runtime.serviceName === ready.topology.serviceName && deployed.runtime.containerPort === ready.topology.containerPort &&
    deployed.runtime.dockerContextClass === ready.topology.dockerContextClass && !/^0{64}$/u.test(deployed.runtime.containerIdentitySha256) &&
    deployed.runtime.dockerEndpointBinding === ready.topology.dockerEndpointBinding &&
    deployed.runtime.runtimeTopologyBinding === ready.topology.runtimeTopologyBinding &&
    deployed.configuration.composeBinding === ready.configuration.candidateComposeBinding &&
    deployed.configuration.environmentBinding === ready.configuration.environmentBinding && deployed.configuration.caddyBinding === ready.configuration.caddyBinding;
}
function sameRollback(rollback: z.infer<typeof rollbackProofSchema>, ready: HostedCutoverReadiness): boolean {
  return sameImage(rollback.runtime.image, ready.rollback.image) && rollback.runtime.serviceName === ready.topology.serviceName &&
    rollback.runtime.containerPort === ready.topology.containerPort && rollback.runtime.dockerContextClass === ready.topology.dockerContextClass &&
    rollback.runtime.dockerEndpointBinding === ready.topology.dockerEndpointBinding &&
    rollback.runtime.runtimeTopologyBinding === ready.topology.runtimeTopologyBinding &&
    rollback.configuration.composeBinding === ready.configuration.baselineComposeBinding && rollback.configuration.environmentBinding === ready.configuration.environmentBinding &&
    rollback.configuration.caddyBinding === ready.configuration.caddyBinding && rollback.runtime.revision === null &&
    !/^0{64}$/u.test(rollback.runtime.containerIdentitySha256) && completeLegacyEndpoint(rollback.endpoint, "rollback-production");
}
function sameRetainedBaseline(baseline: z.infer<typeof retainedBaselineSchema>, ready: HostedCutoverReadiness): boolean {
  return sameImage(baseline.runtime.image, ready.rollback.image) && baseline.runtime.revision === null &&
    baseline.runtime.containerIdentitySha256 === ready.baselineContainerIdentitySha256 &&
    !/^0{64}$/u.test(baseline.runtime.containerIdentitySha256) && baseline.runtime.serviceName === ready.topology.serviceName &&
    baseline.runtime.containerPort === ready.topology.containerPort && baseline.runtime.dockerContextClass === ready.topology.dockerContextClass &&
    baseline.runtime.dockerEndpointBinding === ready.topology.dockerEndpointBinding &&
    baseline.runtime.runtimeTopologyBinding === ready.topology.runtimeTopologyBinding &&
    baseline.runtime.lifecycleBinding === ready.topology.baselineLifecycleBinding &&
    baseline.configuration.composeBinding === ready.configuration.baselineComposeBinding &&
    baseline.configuration.environmentBinding === ready.configuration.environmentBinding && baseline.configuration.caddyBinding === ready.configuration.caddyBinding &&
    completeLegacyEndpoint(baseline.endpoint, "baseline-production");
}
function completeReplacementEndpoint(value: z.infer<typeof replacementEndpointSchema>, role: "candidate" | "production"): boolean {
  return value.role === role && originMatchesRole(value.targetOrigin, role) && exactGates(value.gates, HOSTED_ENDPOINT_GATES);
}
function completeLegacyEndpoint(value: z.infer<typeof legacyEndpointSchema>, role: "baseline" | "rollback" | "baseline-production" | "rollback-production"): boolean {
  return value.role === role && originMatchesRole(value.targetOrigin, role) && exactGates(value.gates, LEGACY_ROLLBACK_GATES);
}
function originMatchesRole(origin: string, role: string): boolean {
  return role === "production" || role.endsWith("-production")
    ? origin === "https://tv.ponbac.xyz"
    : /^http:\/\/(127\.0\.0\.1|localhost):([1-9][0-9]{0,4})$/u.test(origin);
}
function exactGates(actual: readonly { readonly id: string; readonly result: "passed" }[], expected: readonly string[]): boolean {
  return actual.length === expected.length && actual.every((gate, index) => gate.id === expected[index] && gate.result === "passed");
}
function sameImage(left: HostedImageBinding, right: HostedImageBinding): boolean { return left.reference === right.reference && left.digest === right.digest; }
function sameTopology(left: z.infer<typeof topologySchema>, right: z.infer<typeof topologySchema>): boolean { return JSON.stringify(left) === JSON.stringify(right); }
function samePrivateConfiguration(left: z.infer<typeof privateConfigurationSchema>, right: z.infer<typeof privateConfigurationSchema>): boolean { return JSON.stringify(left) === JSON.stringify(right); }
function sameCandidate(left: AcceptanceCandidateBinding, right: AcceptanceCandidateBinding): boolean { return JSON.stringify(left) === JSON.stringify(right); }
function ordered(values: readonly string[]): boolean { return values.every((value, index) => index === 0 || Date.parse(value) >= Date.parse(values[index - 1] ?? "")); }
function within(value: string, start: string, end: string): boolean { const time = Date.parse(value); return time >= Date.parse(start) && time <= Date.parse(end); }
function accept<Value>(value: Value): ParseResult<Value> { return { ok: true, value }; }
function reject(reason: string): ParseResult<never> { return { ok: false, reason }; }
