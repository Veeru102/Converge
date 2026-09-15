import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { hex, unhex } from "@converge/engine";
import {
  clientMsgFromJson,
  clientMsgToJson,
  decodeClient,
  decodeServer,
  encodeClient,
  encodeServer,
  serverMsgFromJson,
  serverMsgToJson,
  type ClientMsgJson,
  type ServerMsgJson,
} from "../src/index.js";

const fx = JSON.parse(
  readFileSync(join(import.meta.dirname, "../../../../fixtures/wire.json"), "utf8"),
) as {
  client: Array<{ json: ClientMsgJson; bytes: string }>;
  server: Array<{ json: ServerMsgJson; bytes: string }>;
};

describe("wire codec matches prost byte for byte", () => {
  it("client messages", () => {
    for (const c of fx.client) {
      const m = clientMsgFromJson(c.json);
      expect(hex(encodeClient(m))).toBe(c.bytes);
      expect(clientMsgToJson(decodeClient(unhex(c.bytes)))).toEqual(c.json);
    }
  });
  it("server messages", () => {
    for (const s of fx.server) {
      const m = serverMsgFromJson(s.json);
      expect(hex(encodeServer(m))).toBe(s.bytes);
      expect(serverMsgToJson(decodeServer(unhex(s.bytes)))).toEqual(s.json);
    }
  });
  it("rejects garbage", () => {
    expect(() => decodeServer(new Uint8Array([255, 255, 255]))).toThrow();
  });
});
