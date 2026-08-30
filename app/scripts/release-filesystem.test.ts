import {
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  rename,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  prepareReleaseOutput,
  snapshotReleaseFiles,
  writeReleasePrivateDirectory,
  writeReleasePrivateFile,
} from "./release-filesystem.ts";

const temporaryRoots: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryRoots
      .splice(0)
      .map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe("release filesystem trust boundary", () => {
  it("rejects a symlinked candidate root", async () => {
    const root = await temporaryRoot();
    const candidate = join(root, "candidate");
    await mkdir(candidate);
    await writeFile(join(candidate, "artifact"), "trusted");
    const linked = join(root, "linked-candidate");
    await symlink(candidate, linked, "dir");

    await expect(
      snapshotReleaseFiles(linked, ["artifact"], { exact: true }),
    ).rejects.toThrow(/directory/u);
  });

  it("rejects a symlinked expected candidate entry", async () => {
    const root = await temporaryRoot();
    const candidate = join(root, "candidate");
    await mkdir(candidate);
    await writeFile(join(root, "outside"), "untrusted");
    await symlink(join(root, "outside"), join(candidate, "artifact"));

    await expect(
      snapshotReleaseFiles(candidate, ["artifact"], { exact: true }),
    ).rejects.toThrow(/entry/u);
  });

  it("rejects a symlinked output ancestor even when it aliases outside containment", async () => {
    const root = await temporaryRoot();
    const candidate = join(root, "candidate");
    const outside = join(root, "outside");
    await Promise.all([mkdir(candidate), mkdir(outside)]);
    const linked = join(root, "linked-output");
    await symlink(outside, linked, "dir");

    await expect(
      prepareReleaseOutput(join(linked, "evidence"), [candidate]),
    ).rejects.toThrow(/ancestor/u);
    await expect(
      prepareReleaseOutput(join(candidate, "evidence"), [candidate]),
    ).rejects.toThrow(/outside/u);
  });

  it("binds snapshots and output writes to open directories across path replacement", async () => {
    const root = await temporaryRoot();
    const candidate = join(root, "candidate");
    await mkdir(candidate);
    await writeFile(join(candidate, "artifact"), "trusted");
    const snapshot = await snapshotReleaseFiles(candidate, ["artifact"], {
      exact: true,
    });
    try {
      await rename(candidate, join(root, "moved-candidate"));
      await mkdir(candidate);
      await writeFile(join(candidate, "artifact"), "substituted");
      expect(
        await readFile(join(snapshot.boundDirectory, "artifact"), "utf8"),
      ).toBe("trusted");
    } finally {
      await snapshot.close();
    }

    const outputParent = join(root, "output-parent");
    const target = await prepareReleaseOutput(
      join(outputParent, "evidence.json"),
      [],
    );
    try {
      const movedParent = join(root, "moved-output-parent");
      await rename(outputParent, movedParent);
      await mkdir(outputParent);
      await writeReleasePrivateFile(target, "trusted-output");
      expect(await readFile(join(movedParent, "evidence.json"), "utf8")).toBe(
        "trusted-output",
      );
      await expect(
        readFile(join(outputParent, "evidence.json"), "utf8"),
      ).rejects.toThrow();
    } finally {
      await target.close();
    }
  });

  it("publishes complete directories without replacing a racing output and permits a clean retry", async () => {
    const root = await temporaryRoot();
    const output = join(root, "sealed");
    const prepared = await prepareReleaseOutput(output, []);
    let parentSyncs = 0;
    const target = {
      ...prepared,
      syncParent: async () => {
        parentSyncs += 1;
        await prepared.syncParent();
      },
    };
    try {
      await expect(
        writeReleasePrivateDirectory(
          target,
          { "verdict.json": "trusted" },
          {
            testHooks: {
              beforeDirectoryPublish: async () => mkdir(output),
            },
          },
        ),
      ).rejects.toThrow(/identity|atomically/u);
      expect(parentSyncs).toBe(0);
      expect(await readdir(root)).toEqual(["sealed"]);
      await rm(output, { recursive: true });
      await writeReleasePrivateDirectory(target, {
        "verdict.json": "trusted",
        "receipt.txt": "complete",
      });
      expect(await readFile(join(output, "verdict.json"), "utf8")).toBe(
        "trusted",
      );
      expect((await readdir(root)).sort()).toEqual(["sealed"]);
      expect(parentSyncs).toBe(1);
    } finally {
      await prepared.close();
    }
  });

  it("publishes a complete file without replacing a racing output and fsyncs its held parent", async () => {
    const root = await temporaryRoot();
    const output = join(root, "continuity.json");
    const prepared = await prepareReleaseOutput(output, []);
    let parentSyncs = 0;
    const target = {
      ...prepared,
      syncParent: async () => {
        parentSyncs += 1;
        await prepared.syncParent();
      },
    };
    try {
      await expect(
        writeReleasePrivateFile(target, "trusted", {
          testHooks: {
            beforeFilePublish: async () => writeFile(output, "racing"),
          },
        }),
      ).rejects.toThrow(/identity/u);
      expect(await readFile(output, "utf8")).toBe("racing");
      expect(parentSyncs).toBe(0);
      expect((await readdir(root)).sort()).toEqual(["continuity.json"]);

      await rm(output);
      await writeReleasePrivateFile(target, "trusted");
      expect(await readFile(output, "utf8")).toBe("trusted");
      expect(parentSyncs).toBe(1);
      expect((await readdir(root)).sort()).toEqual(["continuity.json"]);
    } finally {
      await prepared.close();
    }
  });

  it("closes every descriptor and removes private temporaries after injected failures", async () => {
    const root = await temporaryRoot();
    const candidate = join(root, "candidate");
    await mkdir(candidate);
    await writeFile(join(candidate, "artifact"), "trusted");
    const baseline = await descriptorCount();

    await expect(
      snapshotReleaseFiles(candidate, ["artifact"], {
        exact: true,
        testHooks: {
          afterDestinationOpen: () => {
            throw new Error("injected destination failure");
          },
        },
      }),
    ).rejects.toThrow(/injected destination/u);
    expect(await descriptorCount()).toBe(baseline);

    await expect(
      snapshotReleaseFiles(candidate, ["artifact"], {
        exact: true,
        testHooks: {
          afterSnapshotDirectoryOpen: () => {
            throw new Error("injected chmod failure");
          },
        },
      }),
    ).rejects.toThrow(/injected chmod/u);
    expect(await descriptorCount()).toBe(baseline);

    const target = await prepareReleaseOutput(join(root, "evidence"), []);
    const withTarget = await descriptorCount();
    try {
      await expect(
        writeReleasePrivateDirectory(
          target,
          { "evidence.json": "trusted" },
          {
            testHooks: {
              afterTemporaryDirectoryOpen: () => {
                throw new Error("injected directory failure");
              },
            },
          },
        ),
      ).rejects.toThrow(/injected directory/u);
      expect(await descriptorCount()).toBe(withTarget);
      expect((await readdir(root)).sort()).toEqual(["candidate"]);
    } finally {
      await target.close();
    }

    const fileTarget = await prepareReleaseOutput(
      join(root, "continuity.json"),
      [],
    );
    const withFileTarget = await descriptorCount();
    try {
      await expect(
        writeReleasePrivateFile(fileTarget, "trusted", {
          testHooks: {
            afterTemporaryFileOpen: () => {
              throw new Error("injected file write failure");
            },
          },
        }),
      ).rejects.toThrow(/injected file write/u);
      expect(await descriptorCount()).toBe(withFileTarget);
      expect((await readdir(root)).sort()).toEqual(["candidate"]);
    } finally {
      await fileTarget.close();
    }
  });
});

async function temporaryRoot(): Promise<string> {
  const root = await mkdtemp(
    join(tmpdir(), "sparrow-release-filesystem-test-"),
  );
  temporaryRoots.push(root);
  return root;
}

async function descriptorCount(): Promise<number> {
  return (await readdir(`/proc/${process.pid}/fd`)).length;
}
