import { constants } from "node:fs";
import { randomUUID } from "node:crypto";
import {
  chmod,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readdir,
  realpath,
  rm,
} from "node:fs/promises";
import type { FileHandle } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import {
  basename,
  dirname,
  isAbsolute,
  join,
  parse,
  relative,
  resolve,
  sep,
} from "node:path";

const COPY_BUFFER_BYTES = 1024 * 1024;
const REGULAR_READ_LIMIT = 4 * 1024 * 1024;
const READ_NOFOLLOW = constants.O_RDONLY | constants.O_NOFOLLOW;
const DIRECTORY_NOFOLLOW = READ_NOFOLLOW | constants.O_DIRECTORY;
const WRITE_NEW_NOFOLLOW =
  constants.O_WRONLY |
  constants.O_CREAT |
  constants.O_EXCL |
  constants.O_NOFOLLOW;

/** A private regular-file snapshot rooted in an open directory handle. */
export interface ReleaseDirectorySnapshot {
  readonly sourceDirectory: string;
  readonly directory: string;
  readonly boundDirectory: string;
  close(): Promise<void>;
}

/** A missing output entry whose parent remains bound by an open directory handle. */
export interface ReleaseOutputTarget {
  readonly requestedPath: string;
  readonly boundPath: string;
  syncParent(): Promise<void>;
  close(): Promise<void>;
}

/** Failure injection points used only by focused descriptor-cleanup regressions. */
export interface ReleaseFilesystemTestHooks {
  afterDestinationOpen?(): void | Promise<void>;
  afterSnapshotDirectoryOpen?(): void | Promise<void>;
  afterTemporaryDirectoryOpen?(): void | Promise<void>;
  beforeDirectoryPublish?(): void | Promise<void>;
  afterTemporaryFileOpen?(): void | Promise<void>;
  beforeFilePublish?(): void | Promise<void>;
  beforeParentSync?(): void | Promise<void>;
}

/** A safe filesystem rejection suitable for translation at a release CLI boundary. */
export class ReleaseFilesystemFailure extends Error {
  readonly _tag = "ReleaseFilesystemFailure";
}

/** Reads one no-follow regular file and rejects changes observed across the read. */
export async function readReleaseRegularFile(
  path: string,
  maximumBytes = REGULAR_READ_LIMIT,
): Promise<Buffer> {
  const handle = await openRegularNoFollow(path);
  try {
    const before = await regularIdentity(handle, maximumBytes);
    const contents = await handle.readFile();
    const after = await regularIdentity(handle, maximumBytes);
    if (
      !sameIdentity(before, after) ||
      BigInt(contents.byteLength) !== before.size
    ) {
      throw new ReleaseFilesystemFailure(
        "a release file changed while it was being read",
      );
    }
    return contents;
  } finally {
    await handle.close();
  }
}

/** Snapshots named regular files through held no-follow handles into a private directory. */
export async function snapshotReleaseFiles(
  inputDirectory: string,
  expectedNames: readonly string[],
  options: {
    readonly exact: boolean;
    readonly testHooks?: ReleaseFilesystemTestHooks;
  },
): Promise<ReleaseDirectorySnapshot> {
  const source = await openCanonicalDirectory(inputDirectory);
  let temporary: string | undefined;
  try {
    const names = validateEntryNames(expectedNames);
    if (options.exact) {
      const actual = (await readdir(directoryHandlePath(source.handle))).sort();
      const expected = [...names].sort();
      if (
        actual.length !== expected.length ||
        !actual.every((name, index) => name === expected[index])
      ) {
        throw new ReleaseFilesystemFailure(
          "the release directory has unexpected or missing files",
        );
      }
    }

    temporary = await mkdtemp(join(tmpdir(), "sparrow-release-snapshot-"));
    await chmod(temporary, 0o700);
    for (const name of names) {
      await copyRegularFile(
        join(directoryHandlePath(source.handle), name),
        join(temporary, name),
        options.testHooks,
      );
    }
    const snapshot = await openCanonicalDirectory(temporary);
    try {
      await options.testHooks?.afterSnapshotDirectoryOpen?.();
      await chmod(temporary, 0o500);
      const cleanupPath = temporary;
      temporary = undefined;
      return {
        sourceDirectory: source.canonicalPath,
        directory: cleanupPath,
        boundDirectory: directoryHandlePath(snapshot.handle),
        close: async () => {
          await snapshot.handle.close();
          await chmod(cleanupPath, 0o700).catch(() => undefined);
          await rm(cleanupPath, { recursive: true, force: true });
        },
      };
    } catch (error) {
      await snapshot.handle.close();
      throw error;
    }
  } finally {
    await source.handle.close();
    if (temporary !== undefined)
      await rm(temporary, { recursive: true, force: true });
  }
}

/** Resolves a new output below no symlink ancestors and outside forbidden canonical roots. */
export async function prepareReleaseOutput(
  input: string,
  forbiddenRoots: readonly string[],
): Promise<ReleaseOutputTarget> {
  const requestedPath = resolve(input);
  if (forbiddenRoots.some((root) => isInside(requestedPath, root))) {
    throw new ReleaseFilesystemFailure(
      "release output must be outside the candidate directory",
    );
  }
  const name = basename(requestedPath);
  if (!safeEntryName(name)) {
    throw new ReleaseFilesystemFailure("the release output name is invalid");
  }
  const parent = await openOrCreateDirectoryTree(dirname(requestedPath));
  try {
    const boundPath = join(directoryHandlePath(parent.handle), name);
    const existing = await lstat(boundPath).catch((error: unknown) => {
      if (hasCode(error, "ENOENT")) return undefined;
      throw new ReleaseFilesystemFailure(
        "the release output cannot be inspected",
      );
    });
    if (existing !== undefined) {
      throw new ReleaseFilesystemFailure("the release output already exists");
    }
    return {
      requestedPath,
      boundPath,
      syncParent: () => parent.handle.sync(),
      close: async () => parent.handle.close(),
    };
  } catch (error) {
    await parent.handle.close();
    throw error;
  }
}

/** Atomically reserves and writes a new private regular output file. */
export async function writeReleasePrivateFile(
  target: ReleaseOutputTarget,
  contents: string,
  options: { readonly testHooks?: ReleaseFilesystemTestHooks } = {},
): Promise<void> {
  const temporary = join(
    dirname(target.boundPath),
    `.${basename(target.boundPath)}.sparrow-private-${randomUUID()}`,
  );
  let handle: FileHandle | undefined;
  try {
    handle = await open(temporary, WRITE_NEW_NOFOLLOW, 0o600).catch(() => {
      throw new ReleaseFilesystemFailure(
        "the temporary release output file cannot be created",
      );
    });
    await handle.chmod(0o600);
    const identity = await handle.stat({ bigint: true });
    const pathIdentity = await lstat(temporary, { bigint: true });
    if (
      !identity.isFile() ||
      identity.dev !== pathIdentity.dev ||
      identity.ino !== pathIdentity.ino
    ) {
      throw new ReleaseFilesystemFailure(
        "the temporary release output file identity is invalid",
      );
    }
    await options.testHooks?.afterTemporaryFileOpen?.();
    await handle.writeFile(contents, "utf8");
    await handle.sync();
    await options.testHooks?.beforeFilePublish?.();
    await requirePathIdentity(temporary, identity, "file");
    publishNoReplace(temporary, target.boundPath);
    const published = await openRegularNoFollow(target.boundPath);
    try {
      const publishedIdentity = await published.stat({ bigint: true });
      if (
        publishedIdentity.dev !== identity.dev ||
        publishedIdentity.ino !== identity.ino
      ) {
        throw new ReleaseFilesystemFailure(
          "the completed release output file identity changed during publication",
        );
      }
    } finally {
      await published.close();
    }
    await options.testHooks?.beforeParentSync?.();
    await target.syncParent();
  } finally {
    await handle?.close();
    await rm(temporary, { force: true });
  }
}

/** Atomically reserves a private output directory and writes only new regular entries. */
export async function writeReleasePrivateDirectory(
  target: ReleaseOutputTarget,
  files: Readonly<Record<string, string>>,
  options: { readonly testHooks?: ReleaseFilesystemTestHooks } = {},
): Promise<void> {
  const parentPath = dirname(target.boundPath);
  const outputName = basename(target.boundPath);
  const temporary = await mkdtemp(
    join(parentPath, `.${outputName}.sparrow-private-`),
    { encoding: "utf8" },
  );
  await chmod(temporary, 0o700).catch(async () => {
    await rm(temporary, { recursive: true, force: true });
    throw new ReleaseFilesystemFailure(
      "the temporary release output cannot be secured",
    );
  });
  let directory: FileHandle | undefined;
  try {
    directory = await open(temporary, DIRECTORY_NOFOLLOW).catch(() => {
      throw new ReleaseFilesystemFailure(
        "the temporary release output cannot be opened safely",
      );
    });
    const identity = await directory.stat({ bigint: true });
    const pathIdentity = await lstat(temporary, { bigint: true });
    if (
      !identity.isDirectory() ||
      identity.dev !== pathIdentity.dev ||
      identity.ino !== pathIdentity.ino
    ) {
      throw new ReleaseFilesystemFailure(
        "the temporary release output identity is invalid",
      );
    }
    await options.testHooks?.afterTemporaryDirectoryOpen?.();
    for (const [name, contents] of Object.entries(files)) {
      if (!safeEntryName(name)) {
        throw new ReleaseFilesystemFailure(
          "a release output entry name is invalid",
        );
      }
      const file = await open(
        join(directoryHandlePath(directory), name),
        WRITE_NEW_NOFOLLOW,
        0o600,
      );
      try {
        await file.writeFile(contents, "utf8");
        await file.sync();
      } finally {
        await file.close();
      }
    }
    await directory.sync();
    await options.testHooks?.beforeDirectoryPublish?.();
    await requirePathIdentity(temporary, identity, "directory");
    publishNoReplace(temporary, target.boundPath);
    const published = await open(target.boundPath, DIRECTORY_NOFOLLOW).catch(
      () => {
        throw new ReleaseFilesystemFailure(
          "the completed release output cannot be reopened safely",
        );
      },
    );
    try {
      const publishedIdentity = await published.stat({ bigint: true });
      if (
        publishedIdentity.dev !== identity.dev ||
        publishedIdentity.ino !== identity.ino
      ) {
        throw new ReleaseFilesystemFailure(
          "the completed release output identity changed during publication",
        );
      }
    } finally {
      await published.close();
    }
    await options.testHooks?.beforeParentSync?.();
    await target.syncParent();
  } finally {
    await directory?.close();
    await rm(temporary, { recursive: true, force: true });
  }
}

interface OpenDirectory {
  readonly canonicalPath: string;
  readonly handle: FileHandle;
}

interface RegularIdentity {
  readonly dev: bigint;
  readonly ino: bigint;
  readonly size: bigint;
  readonly mtimeNs: bigint;
  readonly ctimeNs: bigint;
  readonly mode: bigint;
}

async function openCanonicalDirectory(input: string): Promise<OpenDirectory> {
  const requested = resolve(input);
  const handle = await open(requested, DIRECTORY_NOFOLLOW).catch(() => {
    throw new ReleaseFilesystemFailure(
      "the release directory is missing, linked, or invalid",
    );
  });
  try {
    const status = await handle.stat();
    if (!status.isDirectory()) {
      throw new ReleaseFilesystemFailure("the release path is not a directory");
    }
    const canonicalPath = await realpath(directoryHandlePath(handle));
    if (canonicalPath !== requested) {
      throw new ReleaseFilesystemFailure(
        "the release directory path is not canonical",
      );
    }
    return { canonicalPath, handle };
  } catch (error) {
    await handle.close();
    throw error;
  }
}

async function openOrCreateDirectoryTree(
  input: string,
): Promise<OpenDirectory> {
  const requested = resolve(input);
  const { root } = parse(requested);
  let current = await open(root, DIRECTORY_NOFOLLOW);
  let canonicalPath = root;
  try {
    const remainder = relative(root, requested);
    for (const name of remainder.length === 0 ? [] : remainder.split(sep)) {
      if (!safeEntryName(name)) {
        throw new ReleaseFilesystemFailure(
          "a release output ancestor name is invalid",
        );
      }
      const childPath = join(directoryHandlePath(current), name);
      const entry = await lstat(childPath).catch((error: unknown) => {
        if (hasCode(error, "ENOENT")) return undefined;
        throw new ReleaseFilesystemFailure(
          "a release output ancestor cannot be inspected",
        );
      });
      if (entry === undefined) {
        await mkdir(childPath, { mode: 0o700 }).catch((error: unknown) => {
          if (!hasCode(error, "EEXIST")) {
            throw new ReleaseFilesystemFailure(
              "a release output ancestor cannot be created",
            );
          }
        });
      } else if (entry.isSymbolicLink() || !entry.isDirectory()) {
        throw new ReleaseFilesystemFailure(
          "a release output ancestor is linked or not a directory",
        );
      }
      const next = await open(childPath, DIRECTORY_NOFOLLOW).catch(() => {
        throw new ReleaseFilesystemFailure(
          "a release output ancestor changed while opening",
        );
      });
      await current.close();
      current = next;
      canonicalPath = join(canonicalPath, name);
      if ((await realpath(directoryHandlePath(current))) !== canonicalPath) {
        throw new ReleaseFilesystemFailure(
          "a release output ancestor is not canonical",
        );
      }
    }
    return { canonicalPath, handle: current };
  } catch (error) {
    await current.close();
    throw error;
  }
}

async function openRegularNoFollow(path: string): Promise<FileHandle> {
  const handle = await open(path, READ_NOFOLLOW).catch(() => {
    throw new ReleaseFilesystemFailure(
      "a release entry is missing, linked, or unreadable",
    );
  });
  try {
    if (!(await handle.stat()).isFile()) {
      throw new ReleaseFilesystemFailure(
        "a release entry is not a regular file",
      );
    }
    return handle;
  } catch (error) {
    await handle.close();
    throw error;
  }
}

async function copyRegularFile(
  sourcePath: string,
  destinationPath: string,
  testHooks?: ReleaseFilesystemTestHooks,
): Promise<void> {
  const source = await openRegularNoFollow(sourcePath);
  let destination: FileHandle | undefined;
  try {
    destination = await open(destinationPath, WRITE_NEW_NOFOLLOW, 0o400);
    await testHooks?.afterDestinationOpen?.();
    const before = await regularIdentity(source);
    const buffer = Buffer.allocUnsafe(COPY_BUFFER_BYTES);
    let position = 0;
    while (true) {
      const { bytesRead } = await source.read(
        buffer,
        0,
        buffer.length,
        position,
      );
      if (bytesRead === 0) break;
      let written = 0;
      while (written < bytesRead) {
        const result = await destination.write(
          buffer,
          written,
          bytesRead - written,
          position + written,
        );
        written += result.bytesWritten;
      }
      position += bytesRead;
    }
    const after = await regularIdentity(source);
    if (!sameIdentity(before, after) || BigInt(position) !== before.size) {
      throw new ReleaseFilesystemFailure(
        "a release entry changed while being snapshotted",
      );
    }
    await destination.truncate(position);
    await destination.chmod((before.mode & 0o111n) === 0n ? 0o400 : 0o500);
    await destination.sync();
  } finally {
    await Promise.all([source.close(), destination?.close()]);
  }
}

function publishNoReplace(source: string, destination: string): void {
  // Both entries are children of the same held parent, so EXDEV is impossible.
  // GNU mv uses the platform's no-replace rename where available; `-n` may
  // report success for a collision, which the caller detects by inode identity.
  const result = spawnSync("mv", ["-n", "-T", "--", source, destination], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new ReleaseFilesystemFailure(
      "the completed release output cannot be published atomically",
    );
  }
}

async function requirePathIdentity(
  path: string,
  identity: { readonly dev: bigint; readonly ino: bigint },
  kind: "file" | "directory",
): Promise<void> {
  const current = await lstat(path, { bigint: true }).catch(() => undefined);
  if (
    current === undefined ||
    current.dev !== identity.dev ||
    current.ino !== identity.ino
  ) {
    throw new ReleaseFilesystemFailure(
      `the temporary release output ${kind} changed before publication`,
    );
  }
}

async function regularIdentity(
  handle: FileHandle,
  maximumBytes = Number.MAX_SAFE_INTEGER,
): Promise<RegularIdentity> {
  const status = await handle.stat({ bigint: true });
  if (
    !status.isFile() ||
    status.size < 0n ||
    status.size > BigInt(maximumBytes)
  ) {
    throw new ReleaseFilesystemFailure(
      "a release entry is not a bounded regular file",
    );
  }
  return {
    dev: status.dev,
    ino: status.ino,
    size: status.size,
    mtimeNs: status.mtimeNs,
    ctimeNs: status.ctimeNs,
    mode: status.mode,
  };
}

function sameIdentity(left: RegularIdentity, right: RegularIdentity): boolean {
  return (
    left.dev === right.dev &&
    left.ino === right.ino &&
    left.size === right.size &&
    left.mtimeNs === right.mtimeNs &&
    left.ctimeNs === right.ctimeNs
  );
}

function validateEntryNames(input: readonly string[]): readonly string[] {
  if (
    new Set(input).size !== input.length ||
    input.some((name) => !safeEntryName(name))
  ) {
    throw new ReleaseFilesystemFailure("the release entry names are invalid");
  }
  return input;
}

function safeEntryName(name: string): boolean {
  return (
    name.length > 0 &&
    name.length <= 160 &&
    basename(name) === name &&
    name !== "." &&
    name !== ".."
  );
}

function directoryHandlePath(handle: FileHandle): string {
  return `/proc/${process.pid}/fd/${handle.fd}`;
}

function isInside(path: string, root: string): boolean {
  const fromRoot = relative(resolve(root), path);
  return (
    fromRoot === "" || (!fromRoot.startsWith("..") && !isAbsolute(fromRoot))
  );
}

function hasCode(error: unknown, code: string): boolean {
  return error instanceof Error && "code" in error && error.code === code;
}
