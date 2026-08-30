import { describe, expect, it } from "vitest";
import {
  HOSTED_ENDPOINT_GATES,
  HOSTED_REHEARSAL_FIXTURE_SHA256,
  LEGACY_ROLLBACK_DIGEST,
  LEGACY_ROLLBACK_GATES,
  parseHostedImageReference,
  parseHostedEndpointEvidence,
  prepareHostedCutover,
  sealHostedProductionEvidence,
  verifyHostedCutoverReadiness,
  verifyHostedRehearsal,
  type HostedCutoverPlan,
  type HostedCutoverReadiness,
  type VerifiedHostedRehearsal,
} from "./hosted-cutover-domain.ts";
import type { CandidateManifest, ParseResult } from "./release-contract-domain.ts";

const ACCEPTED_AT = "2026-08-30T12:00:00.000Z";
const PREPARED_AT = "2026-08-30T12:01:00.000Z";
const REHEARSED_AT = "2026-08-30T12:02:00.000Z";
const REHEARSAL_VERIFIED_AT = "2026-08-30T12:03:00.000Z";
const BASELINE_AT = "2026-08-30T12:04:00.000Z";
const READY_AT = "2026-08-30T12:05:00.000Z";
const CUTOVER_AT = "2026-08-30T12:06:00.000Z";
const CUTOVER_DONE_AT = "2026-08-30T12:06:12.100Z";
const SEALED_AT = "2026-08-30T12:06:13.000Z";
const MASTER_COMMIT = "a".repeat(40);
const HOSTED_COMMIT = "b".repeat(40);
const CANDIDATE_DIGEST = `sha256:${"2".repeat(64)}`;
const CANDIDATE_IMAGE = `ghcr.io/ponbac/sparrow@${CANDIDATE_DIGEST}`;
const ROLLBACK_IMAGE = `docker.io/ponbac/sparrow:0.11.4@${LEGACY_ROLLBACK_DIGEST}`;
const FIXTURE_IMAGE = "docker.io/library/python:3.13.15-alpine3.24@sha256:540c7d91f98ff6880174c40e99067bf5941eb54d818a7a5e094d188b196a934d";
const CONTAINER_IDENTITY = "3".repeat(64);
const BINDING = (digit: string): string => `hmac-sha256:${digit.repeat(64)}`;

const CONTRACT = {
  schemaVersion: 1,
  repository: "ponbac/sparrow-tv",
  service: { name: "sparrow", containerPort: 33733 },
  reverseProxy: {
    publicOrigin: "https://tv.ponbac.xyz",
    upstream: "sparrow:33733",
    mutation: "forbidden",
  },
  rollback: {
    baselineTag: "docker.io/ponbac/sparrow:0.11.4",
    immutableDigest: LEGACY_ROLLBACK_DIGEST,
    digestSource: "hosted-checkpoint",
  },
  rehearsalFixture: {
    image: "docker.io/library/python:3.13.15-alpine3.24@sha256:540c7d91f98ff6880174c40e99067bf5941eb54d818a7a5e094d188b196a934d",
    script: "scripts/hosted-rehearsal-fixture.py",
    scriptSha256: HOSTED_REHEARSAL_FIXTURE_SHA256,
  },
};

const CONFIGURATION = {
  baselineComposeBinding: BINDING("4"),
  candidateComposeBinding: BINDING("4"),
  baselineServiceBinding: BINDING("c"),
  candidateServiceBinding: BINDING("d"),
  environmentBinding: BINDING("5"),
  caddyBinding: BINDING("6"),
};

const MANIFEST: CandidateManifest = {
  schemaVersion: 1,
  version: "0.11.5",
  tag: "v0.11.5",
  commit: MASTER_COMMIT,
  repository: "ponbac/sparrow-tv",
  workflowRunId: "12345",
  workflowRunAttempt: 2,
  publishable: true,
  artifacts: {
    appImage: { name: "Sparrow_0.11.5_x86_64.AppImage", sha256: "7".repeat(64) },
    apk: { name: "Sparrow_0.11.5_universal.apk", sha256: "8".repeat(64) },
  },
  android: {
    applicationId: "xyz.ponbac.sparrow",
    versionName: "0.11.5",
    versionCode: 11_005,
    minSdk: 24,
    targetSdk: 36,
    abis: ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"],
    certificateSha256: "9".repeat(64),
  },
};

describe("hosted hard-cutover domain", () => {
  it("rejects mutable images and pins the known 0.11.4 rollback manifest", () => {
    expect(parseHostedImageReference(CANDIDATE_IMAGE)).toEqual({
      ok: true,
      value: { reference: CANDIDATE_IMAGE, digest: CANDIDATE_DIGEST },
    });
    expect(parseHostedImageReference("ghcr.io/ponbac/sparrow:latest").ok).toBe(false);
    expect(completePlan().rollback.image).toEqual({
      reference: ROLLBACK_IMAGE,
      digest: LEGACY_ROLLBACK_DIGEST,
    });
  });

  it("consumes structured #23 acceptance and allows a Compose image override", () => {
    const plan = completePlan();
    expect(plan.replacement.revision).toBe(HOSTED_COMMIT);
    expect(plan.configuration.baselineComposeBinding).toBe(
      plan.configuration.candidateComposeBinding,
    );
    expect(plan.topology).toMatchObject({
      serviceName: "sparrow",
      containerPort: 33733,
      publicOrigin: "https://tv.ponbac.xyz",
      caddyMutation: "forbidden",
      dockerContextClass: "remote-ssh",
    });
  });

  it("requires keyed private bindings rather than reusable raw environment hashes", () => {
    const input = preparation();
    expect(
      prepareHostedCutover({
        ...input,
        trustedFacts: {
          ...input.trustedFacts,
          configuration: {
            ...input.trustedFacts.configuration,
            environmentBinding: "5".repeat(64),
          },
        },
      }).ok,
    ).toBe(false);
  });

  it("uses legacy liveness for baseline/rollback and the complete contract for replacement", () => {
    const plan = completePlan();
    const rehearsal = completeRehearsal(plan);
    expect(rehearsal.steps[0].endpoint.gates.map((gate) => gate.id)).toEqual(
      LEGACY_ROLLBACK_GATES,
    );
    expect(rehearsal.steps[1].endpoint.gates.map((gate) => gate.id)).toEqual(
      HOSTED_ENDPOINT_GATES,
    );
    expect(rehearsal.steps[2].endpoint.gates.map((gate) => gate.id)).toEqual(
      LEGACY_ROLLBACK_GATES,
    );

    const raw = rehearsalObservation(plan);
    expect(
      verifyHostedRehearsal({
        plan,
        observation: {
          ...raw,
          steps: [
            { ...raw.steps[0], endpoint: replacementEndpoint("candidate", REHEARSED_AT) },
            raw.steps[1],
            raw.steps[2],
          ],
        },
        verifiedAt: REHEARSAL_VERIFIED_AT,
      }).ok,
    ).toBe(false);
  });

  it("rejects endpoint origins that do not match their provenance role", () => {
    expect(parseHostedEndpointEvidence({ ...replacementEndpoint("candidate", ACCEPTED_AT), targetOrigin: "https://tv.ponbac.xyz" }).ok).toBe(false);
    expect(parseHostedEndpointEvidence({ ...legacyEndpoint("rollback-production", ACCEPTED_AT), targetOrigin: "http://127.0.0.1:33733" }).ok).toBe(false);
    expect(parseHostedEndpointEvidence({ ...replacementEndpoint("candidate", ACCEPTED_AT), targetOrigin: "http://user:pass@127.0.0.1:33733/path?q=1" }).ok).toBe(false);
  });

  it("accepts #23 hosted bytes as an ancestor of the final accepted release", () => {
    const readiness = completeReadiness();
    expect(readiness.replacement.revision).toBe(HOSTED_COMMIT);
    expect(readiness.masterCommit).toBe(MASTER_COMMIT);
    expect(readiness.hostedAcceptance.revisionRelation).toBe("ancestor");
  });

  it("reopens the exact hosted acceptance bytes at readiness", () => {
    const input = readinessInput();
    expect(verifyHostedCutoverReadiness({ ...input, hostedAcceptanceSha256: "0".repeat(64) }).ok).toBe(false);
    expect(verifyHostedCutoverReadiness({ ...input, hostedAcceptance: { ...input.hostedAcceptance, revision: "c".repeat(40) } }).ok).toBe(false);
    expect(verifyHostedCutoverReadiness({ ...input, workflowEvidence: { ...input.workflowEvidence,
      hostedApproval: { ...input.workflowEvidence.hostedApproval, acceptanceSha256: "1".repeat(64) } } }).ok).toBe(false);
  });

  it("rejects an unpinned rehearsal fixture", () => {
    const input = preparation();
    expect(prepareHostedCutover({ ...input, hostedAcceptance: { ...input.hostedAcceptance,
      fixture: { ...input.hostedAcceptance.fixture, image: image(CANDIDATE_IMAGE) } } }).ok).toBe(false);
  });

  it("rejects a rehearsal fixture script digest that differs from the deployment contract", () => {
    const input = preparation();
    expect(prepareHostedCutover({
      ...input,
      hostedAcceptance: {
        ...input.hostedAcceptance,
        fixture: {
          ...input.hostedAcceptance.fixture,
          scriptSha256: "6".repeat(64),
        },
      },
    }).ok).toBe(false);
  });

  it("rejects a non-ancestor hosted revision, stale attempt, or missing publication approval", () => {
    const input = readinessInput();
    expect(
      verifyHostedCutoverReadiness({
        ...input,
        workflowEvidence: {
          ...input.workflowEvidence,
          hostedRevision: {
            ...input.workflowEvidence.hostedRevision,
            baseCommit: "c".repeat(40),
          },
        },
      }),
    ).toEqual({
      ok: false,
      reason: "the accepted hosted image is not an ancestor of the final replacement",
    });
    expect(
      verifyHostedCutoverReadiness({
        ...input,
        workflowEvidence: {
          ...input.workflowEvidence,
          release: { ...input.workflowEvidence.release, runAttempt: 3 },
        },
      }).ok,
    ).toBe(false);
    expect(
      verifyHostedCutoverReadiness({
        ...input,
        workflowEvidence: {
          ...input.workflowEvidence,
          publicationApproval: {
            verified: true,
            evidenceSha256: "0".repeat(64),
          },
        },
      }).ok,
    ).toBe(false);
  });

  it("rejects stale baseline evidence and private/Caddy drift", () => {
    const input = readinessInput();
    expect(
      verifyHostedCutoverReadiness({
        ...input,
        baselineObservation: {
          ...input.baselineObservation,
          recordedAt: "2026-08-30T11:00:00.000Z",
        },
      }).ok,
    ).toBe(false);
    expect(
      verifyHostedCutoverReadiness({
        ...input,
        baselineObservation: {
          ...input.baselineObservation,
          configuration: {
            ...input.baselineObservation.configuration,
            caddyBinding: BINDING("0"),
          },
        },
      }).ok,
    ).toBe(false);
  });

  it("derives downtime and seals an exact successful production runtime", () => {
    const readiness = completeReadiness();
    const result = sealHostedProductionEvidence({
      readiness,
      readinessSha256: "d".repeat(64),
      observation: successfulObservation(readiness),
      sealedAt: SEALED_AT,
    });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value).toMatchObject({
      verdict: "production-accepted",
      replacementImageDigest: CANDIDATE_DIGEST,
      runtimeImageDigest: CANDIDATE_DIGEST,
      rollbackImageDigest: LEGACY_ROLLBACK_DIGEST,
      downtimeSeconds: 13,
      verification: { result: "passed" },
    });
  });

  it("records registry failure without a fictitious rollback when baseline was untouched", () => {
      const readiness = completeReadiness();
      const result = sealHostedProductionEvidence({
        readiness,
        readinessSha256: "d".repeat(64),
        observation: registryFailureObservation(readiness),
        sealedAt: SEALED_AT,
      });
      expect(result.ok).toBe(true);
      if (!result.ok) return;
      expect(result.value.downtimeSeconds).toBe(0);
      expect(result.value.verdict).toBe("production-baseline-retained");
      expect(result.value.verification.result).toBe("baseline-retained");
      expect(result.value.runtimeImageDigest).toBe(LEGACY_ROLLBACK_DIGEST);
      const restarted = registryFailureObservation(readiness);
      expect(sealHostedProductionEvidence({ readiness, readinessSha256: "d".repeat(64), observation: {
        ...restarted, attempt: { ...restarted.attempt, rollback: { ...restarted.attempt.rollback,
          runtime: { ...restarted.attempt.rollback.runtime, lifecycleBinding: BINDING("9") } } } }, sealedAt: SEALED_AT }).ok).toBe(false);
  });

  it("reports an incident-open outage as ongoing rather than restored", () => {
    const readiness = completeReadiness();
    const observation = {
      schemaVersion: 1 as const, readinessSha256: "d".repeat(64), startedAt: CUTOVER_AT, completedAt: CUTOVER_DONE_AT,
      outage: { startedAt: CUTOVER_AT, restoredAt: null },
      attempt: { result: "recovery-failed" as const, failure: "runtime-crash" as const, incident: { opened: true as const, reference: "INC-OPEN-1234" } },
    };
    const result = sealHostedProductionEvidence({ readiness, readinessSha256: "d".repeat(64), observation, sealedAt: SEALED_AT });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value).toMatchObject({ verdict: "production-recovery-failed", downtimeSeconds: null, observedOutageSeconds: 13 });
    expect(sealHostedProductionEvidence({ readiness, readinessSha256: "d".repeat(64), observation: { ...observation,
      outage: { startedAt: CUTOVER_AT, restoredAt: CUTOVER_DONE_AT } }, sealedAt: SEALED_AT }).ok).toBe(false);
  });

  it(
    "seals recreate failure only after the exact legacy runtime is restored",
    () => {
      const readiness = completeReadiness();
      const result = sealHostedProductionEvidence({
        readiness,
        readinessSha256: "d".repeat(64),
        observation: recreateFailureObservation(readiness),
        sealedAt: SEALED_AT,
      });
      expect(result.ok).toBe(true);
      if (!result.ok) return;
      expect(result.value.verdict).toBe("production-rolled-back");
      expect(result.value.runtimeImageDigest).toBe(LEGACY_ROLLBACK_DIGEST);
    },
  );

  it.each([
    "healthcheck-failed",
    "topology-mismatch",
    "runtime-crash",
    "hosted-endpoint-verification-failed",
  ] as const)("seals %s only after verified rollback", (failure) => {
    const readiness = completeReadiness();
    const result = sealHostedProductionEvidence({
      readiness,
      readinessSha256: "d".repeat(64),
      observation: failedAfterStartObservation(readiness, failure),
      sealedAt: SEALED_AT,
    });
    expect(result.ok).toBe(true);
  });

  it("rejects false success, substituted runtime, unverified rollback, and timestamp drift", () => {
    const readiness = completeReadiness();
    const success = successfulObservation(readiness);
    expect(
      sealHostedProductionEvidence({
        readiness,
        readinessSha256: "d".repeat(64),
        observation: {
          ...success,
          attempt: {
            ...success.attempt,
            endpoint: {
              ...success.attempt.endpoint,
              gates: success.attempt.endpoint.gates.slice(1),
            },
          },
        },
        sealedAt: SEALED_AT,
      }).ok,
    ).toBe(false);
    const failure = recreateFailureObservation(readiness);
    expect(
      sealHostedProductionEvidence({
        readiness,
        readinessSha256: "d".repeat(64),
        observation: {
          ...failure,
          attempt: {
            ...failure.attempt,
            rollback: {
              ...failure.attempt.rollback,
              runtime: {
                ...failure.attempt.rollback.runtime,
                image: readiness.replacement.image,
              },
            },
          },
        },
        sealedAt: SEALED_AT,
      }).ok,
    ).toBe(false);
    expect(
      sealHostedProductionEvidence({
        readiness,
        readinessSha256: "d".repeat(64),
        observation: { ...success, completedAt: "2026-08-30T14:00:00.000Z" },
        sealedAt: "2026-08-30T14:00:01.000Z",
      }).ok,
    ).toBe(false);
  });
});

function replacementEndpoint(role: "candidate" | "production", recordedAt: string) {
  return {
    schemaVersion: 1 as const,
    recordedAt,
    targetOrigin: role === "production" ? "https://tv.ponbac.xyz" : "http://127.0.0.1:33733",
    role,
    result: "passed" as const,
    gates: HOSTED_ENDPOINT_GATES.map((id) => ({ id, result: "passed" as const })),
  };
}

function legacyEndpoint(role: "baseline" | "rollback" | "baseline-production" | "rollback-production", recordedAt: string) {
  return {
    schemaVersion: 1 as const,
    recordedAt,
    targetOrigin: role.endsWith("-production") ? "https://tv.ponbac.xyz" : "http://127.0.0.1:33733",
    role,
    result: "passed" as const,
    gates: LEGACY_ROLLBACK_GATES.map((id) => ({ id, result: "passed" as const })),
  };
}

function image(reference: string) {
  return unwrap(parseHostedImageReference(reference));
}

function preparation() {
  return {
    contract: CONTRACT,
    contractSha256: "c".repeat(64),
    hostedAcceptance: {
      schemaVersion: 1,
      verdict: "hosted-accepted",
      recordedAt: ACCEPTED_AT,
      image: image(CANDIDATE_IMAGE),
      revision: HOSTED_COMMIT,
      reproducedManifestDigest: CANDIDATE_DIGEST,
      containerPort: 33733,
      endpoint: replacementEndpoint("candidate", ACCEPTED_AT),
      fixture: {
        image: image(FIXTURE_IMAGE),
        scriptSha256: HOSTED_REHEARSAL_FIXTURE_SHA256,
      },
    },
    hostedAcceptanceSha256: "b".repeat(64),
    preparedAt: PREPARED_AT,
    trustedFacts: {
      configuration: CONFIGURATION,
      replacementImage: image(CANDIDATE_IMAGE),
      baseline: {
        image: image(ROLLBACK_IMAGE),
        containerIdentitySha256: CONTAINER_IDENTITY,
        serviceName: "sparrow",
        containerPort: 33733,
        dockerContextClass: "remote-ssh",
        dockerEndpointBinding: BINDING("a"),
        runtimeTopologyBinding: BINDING("b"),
        lifecycleBinding: BINDING("e"),
      },
      backupsReadable: true,
      replacementRegistryAvailable: true,
      rollbackRegistryAvailable: true,
    },
  };
}

function completePlan(): HostedCutoverPlan {
  return unwrap(prepareHostedCutover(preparation()));
}

function rehearsalObservation(plan: HostedCutoverPlan) {
  return {
    schemaVersion: 1 as const,
    rehearsal: "isolated-baseline-candidate-rollback" as const,
    recordedAt: REHEARSED_AT,
    environmentBinding: plan.configuration.environmentBinding,
    dockerContextClass: "local-unix" as const,
    fixture: plan.fixture,
    steps: [
      {
        role: "baseline" as const,
        image: plan.rollback.image,
        revision: null,
        serviceName: "sparrow" as const,
        containerPort: 33733 as const,
        endpoint: legacyEndpoint("baseline", REHEARSED_AT),
      },
      {
        role: "candidate" as const,
        image: plan.replacement.image,
        revision: plan.replacement.revision,
        serviceName: "sparrow" as const,
        containerPort: 33733 as const,
        endpoint: replacementEndpoint("candidate", REHEARSED_AT),
      },
      {
        role: "rollback" as const,
        image: plan.rollback.image,
        revision: null,
        serviceName: "sparrow" as const,
        containerPort: 33733 as const,
        endpoint: legacyEndpoint("rollback", REHEARSED_AT),
      },
    ] as const,
  };
}

function completeRehearsal(plan: HostedCutoverPlan): VerifiedHostedRehearsal {
  return unwrap(
    verifyHostedRehearsal({
      plan,
      observation: rehearsalObservation(plan),
      verifiedAt: REHEARSAL_VERIFIED_AT,
    }),
  );
}

function workflow(kind: "ci" | "release") {
  return kind === "ci"
    ? {
        workflowName: "CI",
        workflowPath: ".github/workflows/ci.yml",
        runId: "12000",
        runAttempt: 1,
        headSha: MASTER_COMMIT,
        event: "push" as const,
        refName: "master",
        conclusion: "success" as const,
      }
    : {
        workflowName: "Release candidates",
        workflowPath: ".github/workflows/release.yml",
        runId: MANIFEST.workflowRunId,
        runAttempt: MANIFEST.workflowRunAttempt,
        headSha: MASTER_COMMIT,
        event: "push" as const,
        refName: MANIFEST.tag,
        conclusion: "success" as const,
      };
}

function acceptanceCandidate() {
  return {
    schemaVersion: 1 as const,
    repository: MANIFEST.repository,
    version: MANIFEST.version,
    tag: MANIFEST.tag,
    commit: MANIFEST.commit,
    workflowRunId: MANIFEST.workflowRunId,
    workflowRunAttempt: MANIFEST.workflowRunAttempt,
    artifacts: MANIFEST.artifacts,
    android: {
      applicationId: MANIFEST.android.applicationId,
      versionCode: MANIFEST.android.versionCode,
      certificateSha256: MANIFEST.android.certificateSha256,
    },
  };
}

function readinessInput() {
  const plan = completePlan();
  const rehearsal = completeRehearsal(plan);
  const acceptanceVerdictSha256 = "e".repeat(64);
  return {
    plan,
    hostedAcceptance: preparation().hostedAcceptance,
    hostedAcceptanceSha256: plan.hostedAcceptanceSha256,
    planSha256: "d".repeat(64),
    rehearsal,
    rehearsalSha256: "c".repeat(64),
    candidateManifest: MANIFEST,
    candidateManifestSha256: "f".repeat(64),
    acceptanceVerdict: {
      schemaVersion: 1,
      verdict: "evidence-complete",
      sealedAt: REHEARSAL_VERIFIED_AT,
      candidate: acceptanceCandidate(),
      candidateArtifact: { id: "9876", sha256: "1".repeat(64) },
      candidateManifestSha256: "f".repeat(64),
      evidenceSha256: {
        session: "2".repeat(64),
        linux: "3".repeat(64),
        android: "4".repeat(64),
        keyContinuity: "5".repeat(64),
      },
      evidenceRecordedAt: {
        linux: REHEARSAL_VERIFIED_AT,
        android: REHEARSAL_VERIFIED_AT,
        keyContinuity: REHEARSAL_VERIFIED_AT,
      },
    },
    acceptanceVerdictSha256,
    workflowEvidence: {
      schemaVersion: 1,
      repository: "ponbac/sparrow-tv",
      masterCommit: MASTER_COMMIT,
      ci: workflow("ci"),
      release: workflow("release"),
      hostedRevision: {
        baseCommit: HOSTED_COMMIT,
        headCommit: MASTER_COMMIT,
        relation: "ancestor" as const,
      },
      publicationApproval: { verified: true as const, evidenceSha256: acceptanceVerdictSha256 },
      hostedApproval: { issueNumber: 23 as const, issueState: "closed" as const, approver: "ponbac",
        acceptanceSha256: plan.hostedAcceptanceSha256, revision: plan.replacement.revision, imageDigest: plan.replacement.image.digest },
    },
    baselineObservation: {
      schemaVersion: 1,
      recordedAt: BASELINE_AT,
      image: plan.rollback.image,
      containerIdentitySha256: CONTAINER_IDENTITY,
      topology: plan.topology,
      configuration: CONFIGURATION,
      backupsReadable: true,
      replacementRegistryAvailable: true,
      rollbackRegistryAvailable: true,
    },
    masterCommit: MASTER_COMMIT,
    verifiedAt: READY_AT,
  };
}

function completeReadiness(): HostedCutoverReadiness {
  return unwrap(verifyHostedCutoverReadiness(readinessInput()));
}

function candidateDeployment(readiness: HostedCutoverReadiness) {
  return {
    runtime: {
      image: readiness.replacement.image,
      revision: readiness.replacement.revision,
      serviceName: "sparrow" as const,
      containerPort: 33733 as const,
      dockerContextClass: readiness.topology.dockerContextClass,
      dockerEndpointBinding: readiness.topology.dockerEndpointBinding,
      runtimeTopologyBinding: readiness.topology.runtimeTopologyBinding,
      lifecycleBinding: BINDING("f"),
      containerIdentitySha256: "a".repeat(64),
    },
    configuration: {
      composeBinding: readiness.configuration.candidateComposeBinding,
      environmentBinding: readiness.configuration.environmentBinding,
      caddyBinding: readiness.configuration.caddyBinding,
    },
  };
}

function rollbackProof(readiness: HostedCutoverReadiness) {
  return {
    performed: true as const,
    runtime: {
      image: readiness.rollback.image,
      revision: null,
      serviceName: "sparrow" as const,
      containerPort: 33733 as const,
      dockerContextClass: readiness.topology.dockerContextClass,
      dockerEndpointBinding: readiness.topology.dockerEndpointBinding,
      runtimeTopologyBinding: readiness.topology.runtimeTopologyBinding,
      lifecycleBinding: BINDING("0"),
      containerIdentitySha256: "b".repeat(64),
    },
    configuration: {
      composeBinding: readiness.configuration.baselineComposeBinding,
      environmentBinding: readiness.configuration.environmentBinding,
      caddyBinding: readiness.configuration.caddyBinding,
    },
    endpoint: legacyEndpoint("rollback-production", CUTOVER_DONE_AT),
  };
}

function successfulObservation(readiness: HostedCutoverReadiness) {
  return {
    schemaVersion: 1 as const,
    readinessSha256: "d".repeat(64),
    startedAt: CUTOVER_AT,
    completedAt: CUTOVER_DONE_AT,
    outage: { startedAt: CUTOVER_AT, restoredAt: CUTOVER_DONE_AT },
    attempt: {
      result: "passed" as const,
      deployed: candidateDeployment(readiness),
      endpoint: replacementEndpoint("production", CUTOVER_DONE_AT),
    },
  };
}

function registryFailureObservation(readiness: HostedCutoverReadiness) {
  const retained = rollbackProof(readiness);
  return {
    schemaVersion: 1 as const,
    readinessSha256: "d".repeat(64),
    startedAt: CUTOVER_AT,
    completedAt: CUTOVER_DONE_AT,
    outage: { startedAt: null, restoredAt: null },
    attempt: {
      result: "registry-failed" as const,
      failure: "registry-pull-failed" as const,
      deployed: null,
      rollback: {
        ...retained,
        performed: false as const,
        reason: "baseline-unmodified" as const,
        runtime: {
          ...retained.runtime,
          containerIdentitySha256: readiness.baselineContainerIdentitySha256,
          lifecycleBinding: readiness.topology.baselineLifecycleBinding,
        },
        endpoint: legacyEndpoint("baseline-production", CUTOVER_DONE_AT),
      },
    },
  };
}

function recreateFailureObservation(readiness: HostedCutoverReadiness) {
  return {
    schemaVersion: 1 as const,
    readinessSha256: "d".repeat(64),
    startedAt: CUTOVER_AT,
    completedAt: CUTOVER_DONE_AT,
    outage: { startedAt: CUTOVER_AT, restoredAt: CUTOVER_DONE_AT },
    attempt: {
      result: "failed" as const,
      failure: "service-recreate-failed" as const,
      deployed: null,
      rollback: rollbackProof(readiness),
    },
  };
}

function failedAfterStartObservation(
  readiness: HostedCutoverReadiness,
  failure:
    | "healthcheck-failed"
    | "topology-mismatch"
    | "runtime-crash"
    | "hosted-endpoint-verification-failed",
) {
  return {
    schemaVersion: 1 as const,
    readinessSha256: "d".repeat(64),
    startedAt: CUTOVER_AT,
    completedAt: CUTOVER_DONE_AT,
    outage: { startedAt: CUTOVER_AT, restoredAt: CUTOVER_DONE_AT },
    attempt: {
      result: "failed-after-start" as const,
      failure,
      rollback: rollbackProof(readiness),
    },
  };
}

function unwrap<Value>(result: ParseResult<Value>): Value {
  expect(result.ok).toBe(true);
  if (!result.ok) throw new Error(result.reason);
  return result.value;
}
