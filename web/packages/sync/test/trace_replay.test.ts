/**
 * Replays the Rust simulator's per-client traces (`fixtures/sync`) through the
 * TypeScript SyncEngine. Every output, hash and pending-queue state must match
 * the reference engine step by step.
 */
import { blake3 } from "@noble/hashes/blake3";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  hex,
  hlcFromJson,
  opFromJson,
  opToJson,
  unhex,
  type OpJson,
  type HlcJson,
} from "@converge/engine";
import {
  clientMsgToJson,
  serverMsgFromJson,
  userFromJson,
  type ServerMsgJson,
  type UserJson,
} from "@converge/protocol";
import { SyncEngine, type Now, type Output } from "../src/engine.js";

const DIR = join(import.meta.dirname, "../../../../fixtures/sync");

interface NowJson {
  wall: string;
  mono: string;
}
type StepIn =
  | { kind: "connected"; now: NowJson }
  | { kind: "disconnected" }
  | { kind: "tick"; now: NowJson }
  | { kind: "edit"; now: NowJson; op_kind: OpJson["kind"]; op: OpJson }
  | { kind: "persisted"; counter: string; now: NowJson }
  | {
      kind: "snapshot";
      expect: { seq: string; high_water: HlcJson; acked: string[]; snapshot_hash: string };
    }
  | { kind: "snapshot_persisted"; acked: string[] }
  | { kind: "message"; gen: string; now: NowJson; msg: ServerMsgJson }
  | {
      kind: "restore";
      row: { next_counter: string; high_water: HlcJson; clock_offset_ms: string };
      snapshot: { bytes: string; seq: string } | null;
      pending: OpJson[];
    };
interface Step {
  in: StepIn;
  out: unknown[];
  hash: string;
  last_seq: string;
  pending: Array<[string, boolean, boolean, boolean]>;
}
interface Trace {
  scenario: string;
  seed: number;
  client: number;
  doc: string;
  replica: string;
  user: UserJson;
  config: { retimestamp_margin_ms: number; hello_timeout_ms: number; ack_timeout_ms: number };
  steps: Step[];
  final_hash: string;
}

function now(j: NowJson): Now {
  return { wallMs: BigInt(j.wall), monoMs: Number(j.mono) };
}

function outJson(out: Output[]): unknown[] {
  return out.map((o) => {
    switch (o.type) {
      case "send":
        return { send: clientMsgToJson(o.msg) };
      case "persist":
        return { persist: opToJson(o.op) };
      case "reconnect":
        return "reconnect";
    }
  });
}

describe("SyncEngine replays the Rust reference traces", () => {
  const files = readdirSync(DIR)
    .filter((f) => f.endsWith(".json"))
    .sort();
  expect(files.length).toBeGreaterThan(5);
  for (const file of files) {
    it(file, () => {
      const t = JSON.parse(readFileSync(join(DIR, file), "utf8")) as Trace;
      const cfg = {
        retimestampMarginMs: BigInt(t.config.retimestamp_margin_ms),
        helloTimeoutMs: t.config.hello_timeout_ms,
        ackTimeoutMs: t.config.ack_timeout_ms,
      };
      let engine = new SyncEngine(t.doc, BigInt(t.replica), userFromJson(t.user), cfg);
      t.steps.forEach((step, i) => {
        const out: Output[] = [];
        const s = step.in;
        const where = `${file} step ${i} (${s.kind})`;
        switch (s.kind) {
          case "connected":
            engine.connected(now(s.now), out);
            break;
          case "disconnected":
            engine.disconnected();
            break;
          case "tick":
            engine.tick(now(s.now), out);
            break;
          case "edit": {
            const kind = opFromJson(s.op).kind;
            const op = engine.edit(kind, now(s.now), out);
            expect(opToJson(op), where).toEqual(s.op);
            break;
          }
          case "persisted":
            engine.persisted(BigInt(s.counter), now(s.now), out);
            break;
          case "snapshot": {
            const w = engine.snapshot();
            expect(w.seq.toString(), where).toBe(s.expect.seq);
            expect(w.acked.map(String), where).toEqual(s.expect.acked);
            expect(w.highWater, where).toEqual(hlcFromJson(s.expect.high_water));
            expect(hex(blake3(w.bytes)), where).toBe(s.expect.snapshot_hash);
            break;
          }
          case "snapshot_persisted":
            engine.snapshotPersisted(s.acked.map(BigInt));
            break;
          case "message":
            engine.message(Number(s.gen), serverMsgFromJson(s.msg), now(s.now), out);
            break;
          case "restore":
            engine = SyncEngine.restore(
              t.doc,
              BigInt(t.replica),
              userFromJson(t.user),
              cfg,
              {
                nextCounter: BigInt(s.row.next_counter),
                highWater: hlcFromJson(s.row.high_water),
                clockOffsetMs: BigInt(s.row.clock_offset_ms),
              },
              s.snapshot ? { bytes: unhex(s.snapshot.bytes), seq: BigInt(s.snapshot.seq) } : null,
              s.pending.map(opFromJson),
            );
            break;
        }
        expect(outJson(out), where).toEqual(step.out);
        expect(hex(engine.hash()), where).toBe(step.hash);
        expect(engine.lastSeqValue.toString(), where).toBe(step.last_seq);
        expect(
          engine.pendingDebug().map(([c, d, se, a]) => [c.toString(), d, se, a]),
          where,
        ).toEqual(step.pending);
      });
      expect(hex(engine.hash())).toBe(t.final_hash);
    });
  }
});
