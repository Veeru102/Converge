import { compareStamp, STAMP_MIN, type Stamp } from "./hlc.js";
import { compareId, idKey, type ObjectId } from "./ids.js";
import { opObject, opStamp, type Op } from "./op.js";
import { valueAsString, type ObjectKind, type Value } from "./value.js";

export interface Register<T> {
  value: T;
  at: Stamp;
}

function merge<T>(reg: Register<T>, value: T, at: Stamp): boolean {
  if (compareStamp(at, reg.at) > 0) {
    reg.value = value;
    reg.at = at;
    return true;
  }
  return false;
}

export class ObjectState {
  /** `null` while the object is a ghost (ops seen, create not yet seen). */
  kind: ObjectKind | null = null;
  deleted: Register<boolean> = { value: false, at: STAMP_MIN };
  props = new Map<string, Register<Value>>();

  created(): boolean {
    return this.kind !== null;
  }

  visible(): boolean {
    return this.created() && !this.deleted.value;
  }

  get(key: string): Value | undefined {
    return this.props.get(key)?.value;
  }

  /** @internal */
  mergeProp(key: string, value: Value, at: Stamp): boolean {
    const reg = this.props.get(key);
    if (reg) return merge(reg, value, at);
    this.props.set(key, { value, at });
    return true;
  }
}

/**
 * Replicated document. `apply` is commutative, associative and idempotent;
 * see the Rust `Document` for the authoritative semantics.
 */
export class Document {
  private readonly objects = new Map<string, { id: ObjectId; state: ObjectState }>();

  apply(op: Op): boolean {
    const at = opStamp(op);
    const object = opObject(op);
    const key = idKey(object);
    let entry = this.objects.get(key);
    if (!entry) {
      entry = { id: object, state: new ObjectState() };
      this.objects.set(key, entry);
    }
    const s = entry.state;
    const k = op.kind;
    switch (k.op) {
      case "create": {
        let changed = false;
        if (s.kind === null) {
          s.kind = k.kind;
          changed = true;
        }
        for (const [pk, pv] of k.props) changed = s.mergeProp(pk, pv, at) || changed;
        return changed;
      }
      case "set_props": {
        let changed = false;
        for (const [pk, pv] of k.entries) changed = s.mergeProp(pk, pv, at) || changed;
        return changed;
      }
      case "delete":
        return merge(s.deleted, true, at);
      case "restore":
        return merge(s.deleted, false, at);
    }
  }

  get(id: ObjectId): ObjectState | undefined {
    return this.objects.get(idKey(id))?.state;
  }

  get size(): number {
    return this.objects.size;
  }

  /** All objects (ghosts and tombstones included) in canonical id order. */
  entries(): Array<[ObjectId, ObjectState]> {
    const out: Array<[ObjectId, ObjectState]> = [];
    for (const { id, state } of this.objects.values()) out.push([id, state]);
    out.sort((a, b) => compareId(a[0], b[0]));
    return out;
  }

  /** Visible objects in render order: by `z` (missing first), then id. */
  renderOrder(): Array<[ObjectId, ObjectState]> {
    const out = this.entries().filter(([, s]) => s.visible());
    out.sort(([ida, a], [idb, b]) => {
      const za = valueAsString(a.get("z"));
      const zb = valueAsString(b.get("z"));
      if (za === undefined && zb !== undefined) return -1;
      if (za !== undefined && zb === undefined) return 1;
      if (za !== undefined && zb !== undefined && za !== zb) return za < zb ? -1 : 1;
      return compareId(ida, idb);
    });
    return out;
  }

  /** @internal used by the snapshot decoder */
  insertRaw(id: ObjectId, state: ObjectState): void {
    this.objects.set(idKey(id), { id, state });
  }

  clone(): Document {
    const d = new Document();
    for (const [id, s] of this.entries()) {
      const c = new ObjectState();
      c.kind = s.kind;
      c.deleted = { ...s.deleted };
      for (const [k, r] of s.props) c.props.set(k, { ...r });
      d.insertRaw(id, c);
    }
    return d;
  }
}
