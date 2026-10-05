// Runs only against an installed pi. From extensions/, with a built rvow binary:
//   RVOW_NATIVE_SDK=$PWD/node_modules/@earendil-works/pi-coding-agent \
//   RVOW_NATIVE_BIN=<repo>/target/debug/rvow node --test pi/native-codemode.test.ts
// RVOW_NATIVE_BIN defaults to ../target/debug/rvow.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";

const sdkPath = process.env.RVOW_NATIVE_SDK;

test("native SDK codemode tools.bash hits production rvow tool_call before execution", {
  skip: !sdkPath && "set RVOW_NATIVE_SDK to the installed pi-coding-agent package directory",
}, async () => {
  assert.ok(sdkPath);
  const root = mkdtempSync(join(tmpdir(), "rvow-native-"));
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
    // npm nests pi-agent-core under pi-coding-agent; a flat install puts it beside it.
    const nested = join(sdkPath, "node_modules", "@earendil-works", "pi-agent-core");
    const coreDir = existsSync(nested) ? nested : join(dirname(sdkPath), "pi-agent-core");
    const core = await import(pathToFileURL(join(coreDir, "dist", "index.js")).href);
    const seen: { toolName: string; parentToolCallId?: string; input: unknown }[] = [];
    const settingsManager = sdk.SettingsManager.inMemory({
      defaultTools: ["bash", "codemode"],
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
      extensionFactories: [
        sdk.createCodemodeExtension({ models: false }),
        (pi: { on(name: string, handler: (event: (typeof seen)[number]) => void): void }) => {
          pi.on("tool_call", (event) => {
            seen.push(event);
          });
        },
      ],
    });
    await resourceLoader.reload();
    assert.deepEqual(resourceLoader.getExtensions().errors, []);
    const created = await sdk.createAgentSession({
      cwd: repo,
      agentDir,
      resourceLoader,
      settingsManager,
      // An allowlist: tools it leaves out are never registered, so codemode must be named here.
      tools: ["bash", "codemode", "rvow_view"],
      sessionManager: sdk.SessionManager.inMemory(repo),
    });
    session = created.session;
    const native = created.session;
    await native.bindExtensions({});
    native.setActiveToolsByName(["bash", "codemode", "rvow_view"]);
    const agent = native.agent;
    agent.streamFunction = () => {
      throw new Error("no model call is allowed in native tool tests");
    };
    const assistantMessage = {
      role: "assistant",
      content: [],
      api: "openai-responses",
      provider: "test",
      model: "none",
      usage: {
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
      stopReason: "toolUse",
      timestamp: Date.now(),
    };
    // Nested tool calls (codemode's tools.bash) run against the last assistant message in the state.
    agent.state.messages = [assistantMessage];
    const options = {
      tools: agent.state.tools,
      assistantMessage,
      context: { messages: [], tools: agent.state.tools },
      beforeToolCall: agent.beforeToolCall,
      afterToolCall: agent.afterToolCall,
    };
    const command = "rvow accept CHG-native; printf executed > native-executed";
    const denied = await core.runToolCall(
      {
        type: "toolCall",
        id: "native",
        name: "codemode",
        arguments: { code: `await tools.bash(${JSON.stringify({ command })});` },
      },
      options,
    );
    assert.equal(denied.isError, true);
    const output = denied.result.content
      .map((block: { type: string; text?: string }) => block.text ?? "")
      .join("\n");
    assert.match(output, /accept.*confirm.*user|only the user|belong.*user/i);
    assert.equal(existsSync(join(repo, "native-executed")), false);
    // rvow loads first and blocks the nested bash, so a later spy sees only the outer codemode call.
    assert.ok(seen.some((event) => event.toolName === "codemode"));
    const detail = denied.result.details.calls.find(
      (call: { name: string }) => call.name === "bash",
    );
    assert.equal(detail.status, "error");
    assert.match(detail.error, /rvow accept belongs to the user/);
    const recovered = await core.runToolCall(
      {
        type: "toolCall",
        id: "recovered",
        name: "codemode",
        arguments: {
          code: "const view = await tools.rvow_view({target: 'status'}); text(view.kind); text(view.data);",
        },
      },
      options,
    );
    assert.equal(recovered.isError, false);
    const recoveredText = recovered.result.content
      .map((block: { type: string; text?: string }) => block.text ?? "")
      .join("\n");
    assert.match(recoveredText, /status/);
  } finally {
    session?.dispose();
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(root, { recursive: true, force: true });
  }
});
