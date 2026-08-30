import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { describe, expect, it } from "vitest";

const cli = resolve(import.meta.dirname, "hosted-cutover.ts");
const acceptanceRunner = resolve(import.meta.dirname, "../../scripts/accept-hosted-candidate.sh");
const rehearsalRunner = resolve(import.meta.dirname, "../../scripts/rehearse-hosted-cutover.sh");
const endpointVerifier = resolve(import.meta.dirname, "../../scripts/verify-hosted-endpoint.sh");
const rehearsalFixture = resolve(import.meta.dirname, "../../scripts/hosted-rehearsal-fixture.py");

function run(arguments_: readonly string[]) {
  return spawnSync(process.execPath, [cli, ...arguments_], { encoding: "utf8", env: { ...process.env, PATH: "/usr/bin:/bin" } });
}

describe("hosted cutover CLI observation boundary", () => {
  it("snapshots the exact committed rehearsal fixture bytes", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sparrow-cutover-fixture-"));
    const output = join(directory, "fixture.py");
    const snapshot = run(["snapshot-rehearsal-fixture", "--output", output]);
    expect(snapshot.status, snapshot.stderr).toBe(0);
    expect(await readFile(output)).toEqual(await readFile(rehearsalFixture));
  });

  it("creates a keyed start and rejects altered start bytes", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sparrow-cutover-cli-"));
    const readiness = join(directory, "readiness.json"); const key = join(directory, "key"); const start = join(directory, "start.json");
    await writeFile(readiness, "{}\n", { mode: 0o600 }); await writeFile(key, Buffer.from(Array.from({ length: 32 }, (_, index) => index)), { mode: 0o600 });
    const started = run(["start-production-observation", "--readiness", readiness, "--evidence-key", key, "--output", start]);
    expect(started.status, started.stderr).toBe(0);
    const parsed: unknown = JSON.parse(await readFile(start, "utf8"));
    expect(parsed).toMatchObject({ schemaVersion: 1, readinessSha256: expect.stringMatching(/^[0-9a-f]{64}$/u), startHmac: expect.stringMatching(/^[0-9a-f]{64}$/u) });
    const record = parsed as Record<string, unknown>; record.readinessSha256 = "f".repeat(64);
    const altered = join(directory, "altered.json"); await writeFile(altered, JSON.stringify(record), { mode: 0o600 });
    const rejected = run(["finish-production-observation", "--start", altered, "--route-binding", join(directory, "missing"), "--result", "passed", "--failure", "", "--incident-reference", "", "--evidence-key", key, "--output", join(directory, "event")]);
    expect(rejected.status).not.toBe(0);
  });

  it("rejects irrelevant outcome arguments before route consumption", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sparrow-cutover-args-")); const readiness = join(directory, "r"); const key = join(directory, "k"); const start = join(directory, "s");
    await writeFile(readiness, "{}", { mode: 0o600 }); await writeFile(key, Buffer.from(Array.from({ length: 32 }, (_, index) => 255 - index)), { mode: 0o600 });
    const started = run(["start-production-observation", "--readiness", readiness, "--evidence-key", key, "--output", start]);
    expect(started.status, started.stderr).toBe(0);
    expect(run(["finish-production-observation", "--start", start, "--route-binding", join(directory, "missing"), "--result", "passed", "--failure", "bogus", "--incident-reference", "", "--evidence-key", key, "--output", join(directory, "event")]).stderr).toContain("outcome arguments are inconsistent");
  });

  it("rejects predictable or publicly readable evidence keys", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sparrow-cutover-key-")); const readiness = join(directory, "r");
    await writeFile(readiness, "{}", { mode: 0o600 });
    for (const [name, key, mode] of [["zero", Buffer.alloc(32), 0o600], ["public", Buffer.from(Array.from({ length: 32 }, (_, index) => index)), 0o644]] as const) {
      const path = join(directory, name); await writeFile(path, key, { mode });
      await chmod(path, mode);
      const result = run(["start-production-observation", "--readiness", readiness, "--evidence-key", path, "--output", join(directory, `${name}-out`)]);
      expect(result.status, `${name}: ${result.stderr}`).not.toBe(0);
    }
  });

  it("finishes an incident-open observation without fictitious route evidence", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sparrow-cutover-incident-")); const readiness = join(directory, "r"); const key = join(directory, "k"); const start = join(directory, "s"); const event = join(directory, "e");
    await writeFile(readiness, "{}", { mode: 0o600 }); await writeFile(key, Buffer.from(Array.from({ length: 32 }, (_, index) => index + 1)), { mode: 0o600 });
    expect(run(["start-production-observation", "--readiness", readiness, "--evidence-key", key, "--output", start]).status).toBe(0);
    const finished = run(["finish-production-observation", "--start", start, "--route-binding", "", "--result", "recovery-failed", "--failure", "runtime-crash", "--incident-reference", "INC-OPEN-1234", "--evidence-key", key, "--output", event]);
    expect(finished.status, finished.stderr).toBe(0);
    expect(JSON.parse(await readFile(event, "utf8"))).toMatchObject({ result: "recovery-failed", routeBindingSha256: null, incidentReference: "INC-OPEN-1234" });
  });

  it("resolves pinned Bun before narrowing PATH and rejects Docker config overrides", () => {
    const result = spawnSync("/usr/bin/bash", [acceptanceRunner, "x", "x", "x", "x"], {
      encoding: "utf8", env: { ...process.env, DOCKER_CONFIG: "/tmp/attacker-docker-config" },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("DOCKER_CONFIG must be unset");
    expect(result.stderr).not.toContain("bun: command not found");
  });

  it("accepts an inspected Docker IPv4 address outside RFC 1918 ranges", () => {
    const result = spawnSync("/usr/bin/bash", [endpointVerifier, "replacement", "invalid-role", "http://localhost:33733", "/tmp/unused-hosted-evidence"], {
      encoding: "utf8",
      env: { ...process.env, SPARROW_HOSTED_PASSWORD: "synthetic-test-password", SPARROW_HOSTED_RESOLVE_IP: "100.64.0.1" },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("endpoint mode and evidence role are inconsistent");
    expect(result.stderr).not.toContain("inspected Docker IPv4 address");
  });

  it("rejects malformed isolated endpoint addresses", () => {
    const result = spawnSync("/usr/bin/bash", [endpointVerifier, "replacement", "candidate", "http://localhost:33733", "/tmp/unused-hosted-evidence"], {
      encoding: "utf8",
      env: { ...process.env, SPARROW_HOSTED_PASSWORD: "synthetic-test-password", SPARROW_HOSTED_RESOLVE_IP: "999.64.0.1" },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("inspected Docker IPv4 address");
  });

  it("pins Docker operations to the captured endpoint and exact internal network attachment", async () => {
    const acceptance = await readFile(acceptanceRunner, "utf8");
    const rehearsal = await readFile(rehearsalRunner, "utf8");
    expect(acceptance).toContain('docker_cmd() { /usr/bin/docker --host "$docker_endpoint" "$@"; }');
    expect(rehearsal).toContain('docker_cmd() { /usr/bin/docker --host "$context_host" "$@"; }');
    expect(acceptance).toContain('with index .NetworkSettings.Networks \\"$network\\"');
    expect(rehearsal).toContain('with index .NetworkSettings.Networks \\"$network_name\\"');
    for (const runner of [acceptance, rehearsal]) {
      expect(runner).not.toMatch(/^\s*docker\s/mu);
    }
  });
});
