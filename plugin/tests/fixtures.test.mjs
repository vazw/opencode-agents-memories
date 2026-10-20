// Contract fixtures — the frozen wire bytes of tests/fixtures/bridge/.
//
// Written by the `contract` shard (wave 0) and READ-ONLY afterwards. This
// suite exercises the PRODUCTION serializer from plugin/src/plugin.js: there
// is no test-only twin to drift.
//
// The JS lane emits requests only, so six of the eight oracles are request
// byte-equality. The parse-error sample is an error ENVELOPE (the engine's
// canonical error form); its oracle pins the production `encodeLine` to those
// bytes. `ndjson-framing.txt` proves three request lines concatenate exactly.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { encodeLine, serializeRequest } from "../src/plugin.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES = path.join(here, "..", "..", "tests", "fixtures", "bridge");

function read(name) {
  return readFileSync(path.join(FIXTURES, name), "utf8");
}

function doc(name) {
  return JSON.parse(read(name));
}

/** The request fixtures pass their params verbatim; `session_id` is already there. */
function assertRequestByteEqual(name, method) {
  const bytes = read(name);
  const request = doc(name);
  assert.equal(request.method, method);
  assert.equal(
    serializeRequest(request.method, request.params, request.params.session_id, request.id),
    bytes,
  );
}

test("test_fixture_hello_request_byte_equal", () => {
  const bytes = read("hello-request.json");
  const request = doc("hello-request.json");
  assert.equal(request.method, "hello");
  assert.equal(serializeRequest("hello", request.params, undefined, request.id), bytes);
});

test("test_fixture_capture_request_byte_equal", () => {
  assertRequestByteEqual("capture-request.json", "capture");
});

test("test_fixture_inject_request_byte_equal", () => {
  assertRequestByteEqual("inject-request.json", "inject");
});

test("test_fixture_recall_request_byte_equal", () => {
  assertRequestByteEqual("recall-request.json", "recall");
});

test("test_fixture_health_request_byte_equal", () => {
  assertRequestByteEqual("health-request.json", "health");
  assert.equal(doc("health-request.json").params.session_id, "oc-bridge-4242");
});

test("test_fixture_shutdown_request_byte_equal", () => {
  assertRequestByteEqual("shutdown-request.json", "shutdown");
  assert.equal(doc("shutdown-request.json").params.session_id, "oc-bridge-4242");
});

test("test_fixture_error_parse_byte_equal", () => {
  const bytes = read("error-32700.json");
  const envelope = JSON.parse(bytes);
  assert.equal(envelope.error.code, -32700);
  assert.equal(envelope.id, null);
  assert.equal(encodeLine(envelope), bytes);
});

test("test_fixture_ndjson_framing_byte_equal", () => {
  const bytes = read("ndjson-framing.txt");
  const lines = bytes.split("\n").filter((line) => line.length > 0);
  assert.equal(lines.length, 3);
  const encoded = lines
    .map((line) => {
      const request = JSON.parse(line);
      return serializeRequest(request.method, request.params, request.params.session_id, request.id);
    })
    .join("");
  assert.equal(encoded, bytes);
});
