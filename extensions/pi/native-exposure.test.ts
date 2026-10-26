// Runs only against an installed pi. From extensions/, with a built rvow binary:
//   RVOW_NATIVE_SDK=$PWD/node_modules/@earendil-works/pi-coding-agent \
//   RVOW_NATIVE_BIN=<repo>/target/debug/rvow node --test pi/native-exposure.test.ts
// RVOW_NATIVE_BIN defaults to ../target/debug/rvow.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import type { ToolInfo } from "@earendil-works/pi-coding-agent";
import { DIALOG_TOOLS } from "./rvow/typed-tools.ts";

const sdkPath = process.env.RVOW_NATIVE_SDK;
const skip = !sdkPath && "set RVOW_NATIVE_SDK to the installed pi-coding-agent package directory";

async function declared(defaultTools: string[]) {
  assert.ok(sdkPath);
  const root = mkdtempSync(join(tmpdir(), "rvow-exposure-"));
  const repo = join(root, "repo");
  const home = join(root, "home");
  const agentDir = join(home, ".pi", "agent");
  mkdirSync(repo);
  mkdirSync(agentDir, { recursive: true });
  const binary = resolve(process.env.RVOW_NATIVE_BIN ?? "../target/debug/rvow");
  assert.ok(existsSync(binary), `build the real rvow binary first: ${binary}`);
  const variables = {
    HOME: home,
    RVOW_HOME: join(home, ".rvow"),
    RVOW_BIN: binary,
    PI_CODING_AGENT_DIR: agentDir,
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_CONFIG_NOSYSTEM: "1",
    PATH: `${dirname(binary)}:${process.env.PATH}`,
  };
  const previous = Object.fromEntries(Object.keys(variables).map((key) => [key, process.env[key]]));
  Object.assign(process.env, variables);
  let session: { dispose(): void } | undefined;
  try {
    execFileSync("git", ["init", "--quiet"], { cwd: repo });
    execFileSync(
      "git",
      [
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "commit",
        "--allow-empty",
        "-m",
        "fixture",
      ],
      { cwd: repo },
    );
    execFileSync(binary, ["init", "--in-repo"], { cwd: repo });
    const sdk = await import(pathToFileURL(join(sdkPath, "dist", "index.js")).href);
    const settingsManager = sdk.SettingsManager.inMemory({
      defaultTools,
      cacheWarming: "off",
      compaction: { enabled: false },
    });
    const resourceLoader = new sdk.DefaultResourceLoader({
      cwd: repo,
      agentDir,
      settingsManager,
      additionalExtensionPaths: [resolve("pi/rvow/index.ts")],
      noSkills: true,
      noPromptTemplates: true,
      noThemes: true,
      extensionFactories: [sdk.createToolSearchExtension()],
    });
    await resourceLoader.reload();
    assert.deepEqual(resourceLoader.getExtensions().errors, []);
    const created = await sdk.createAgentSession({
      cwd: repo,
      agentDir,
      resourceLoader,
      settingsManager,
      sessionManager: sdk.SessionManager.inMemory(repo),
    });
    session = created.session;
    await created.session.bindExtensions({});
    const rvow = (names: string[]) => names.filter((name) => name.startsWith("rvow_")).sort();
    return {
      declared: rvow(created.session.getActiveToolNames()),
      callable: rvow(created.session.getCallableToolNames()),
      all: created.session.getAllTools() as ToolInfo[],
    };
  } finally {
    session?.dispose();
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(root, { recursive: true, force: true });
  }
}

test("without tool_search, pi declares only the dialog tools", { skip }, async () => {
  assert.deepEqual((await declared(["bash"])).declared, [...DIALOG_TOOLS].sort());
});

test("with tool_search, pi declares no rvow tool until a search loads it", { skip }, async () => {
  const tools = await declared(["bash", "tool_search"]);
  assert.deepEqual(tools.declared, []);
  for (const name of DIALOG_TOOLS) assert.ok(tools.callable.includes(name), name);
});

test("tool search ranks the right rvow tool first for each decision", { skip }, async () => {
  assert.ok(sdkPath);
  const search = await import(
    pathToFileURL(join(sdkPath, "dist", "extensions", "tool-search", "tool.js")).href
  );
  const { all } = await declared(["bash", "tool_search"]);
  const documents = all
    .filter((tool) => tool.exposure === "deferred")
    .map((tool) => search.createToolSearchDocument(tool, tool.namespace));
  const ranker = new search.Bm25Ranker();
  const first = (query: string) => ranker.rank(query, documents, 1)[0]?.name;
  assert.equal(first("accept the plan"), "rvow_accept");
  assert.equal(first("archive the finished change"), "rvow_archive");
  assert.equal(first("confirm amendments"), "rvow_confirm");
  assert.equal(first("open questions"), "rvow_ask");
  assert.equal(first("yolo"), "rvow_yolo");
  assert.equal(first("spec judge"), "rvow_judge");
  assert.equal(ranker.rank("rvow", documents, 8).length, DIALOG_TOOLS.length);
});
