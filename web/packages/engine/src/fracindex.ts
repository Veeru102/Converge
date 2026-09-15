/**
 * Fractional indexing (port of rocicorp's algorithm; identical to the Rust
 * `fracindex` module — `fixtures/fracindex.json` pins both).
 */

const DIGITS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const SMALLEST_INTEGER = "A00000000000000000000000000";
export const FIRST_KEY = "a0";

export class FracIndexError extends Error {}

function midpoint(a: string, b: string | null): string {
  if (b !== null) {
    if (a >= b) throw new FracIndexError(`${a} >= ${b}`);
    let n = 0;
    while (n < b.length && (a[n] ?? "0") === b[n]) n++;
    if (n > 0) return b.slice(0, n) + midpoint(a.slice(n), b.slice(n));
  }
  const digitA = a ? DIGITS.indexOf(a[0]!) : 0;
  const digitB = b !== null ? DIGITS.indexOf(b[0]!) : DIGITS.length;
  if (digitB - digitA > 1) {
    const mid = Math.floor((digitA + digitB + 1) / 2);
    return DIGITS[mid]!;
  }
  if (b !== null && b.length > 1) return b.slice(0, 1);
  return DIGITS[digitA]! + midpoint(a.slice(1), null);
}

function integerLength(head: string): number {
  if (head >= "a" && head <= "z") return head.charCodeAt(0) - "a".charCodeAt(0) + 2;
  if (head >= "A" && head <= "Z") return "Z".charCodeAt(0) - head.charCodeAt(0) + 2;
  throw new FracIndexError(`invalid order key head ${head}`);
}

function integerPart(key: string): string {
  if (!key) throw new FracIndexError("empty key");
  const len = integerLength(key[0]!);
  if (len > key.length) throw new FracIndexError(`invalid order key ${key}`);
  return key.slice(0, len);
}

function validateOrThrow(key: string): void {
  if (key === SMALLEST_INTEGER) throw new FracIndexError(`invalid order key ${key}`);
  for (const c of key)
    if (DIGITS.indexOf(c) < 0) throw new FracIndexError(`invalid char in ${key}`);
  const i = integerPart(key);
  const f = key.slice(i.length);
  if (f.endsWith("0")) throw new FracIndexError(`invalid order key ${key}`);
}

export function validate(key: string): boolean {
  try {
    validateOrThrow(key);
    return true;
  } catch {
    return false;
  }
}

function incrementInteger(x: string): string | null {
  const head = x[0]!;
  const digs = x.slice(1).split("");
  let carry = true;
  for (let i = digs.length - 1; carry && i >= 0; i--) {
    const d = DIGITS.indexOf(digs[i]!) + 1;
    if (d === DIGITS.length) {
      digs[i] = "0";
    } else {
      digs[i] = DIGITS[d]!;
      carry = false;
    }
  }
  if (carry) {
    if (head === "Z") return "a0";
    if (head === "z") return null;
    const h = String.fromCharCode(head.charCodeAt(0) + 1);
    if (h > "a") digs.push("0");
    else digs.pop();
    return h + digs.join("");
  }
  return head + digs.join("");
}

function decrementInteger(x: string): string | null {
  const head = x[0]!;
  const digs = x.slice(1).split("");
  let borrow = true;
  for (let i = digs.length - 1; borrow && i >= 0; i--) {
    const d = DIGITS.indexOf(digs[i]!) - 1;
    if (d === -1) {
      digs[i] = DIGITS[DIGITS.length - 1]!;
    } else {
      digs[i] = DIGITS[d]!;
      borrow = false;
    }
  }
  if (borrow) {
    if (head === "a") return "Z" + DIGITS[DIGITS.length - 1];
    if (head === "A") return null;
    const h = String.fromCharCode(head.charCodeAt(0) - 1);
    if (h < "Z") digs.push(DIGITS[DIGITS.length - 1]!);
    else digs.pop();
    return h + digs.join("");
  }
  return head + digs.join("");
}

/** A key strictly between `a` and `b` (`null` = open end). Throws on bad input. */
export function between(a: string | null, b: string | null): string {
  if (a !== null) validateOrThrow(a);
  if (b !== null) validateOrThrow(b);
  if (a !== null && b !== null && a >= b) throw new FracIndexError(`${a} >= ${b}`);
  if (a === null) {
    if (b === null) return FIRST_KEY;
    const ib = integerPart(b);
    const fb = b.slice(ib.length);
    if (ib === SMALLEST_INTEGER) return ib + midpoint("", fb);
    if (ib < b) return ib;
    const res = decrementInteger(ib);
    if (res === null) throw new FracIndexError("cannot decrement any more");
    return res;
  }
  if (b === null) {
    const ia = integerPart(a);
    const fa = a.slice(ia.length);
    const i = incrementInteger(ia);
    return i === null ? ia + midpoint(fa, null) : i;
  }
  const ia = integerPart(a);
  const fa = a.slice(ia.length);
  const ib = integerPart(b);
  const fb = b.slice(ib.length);
  if (ia === ib) return ia + midpoint(fa, fb);
  const i = incrementInteger(ia);
  if (i === null) throw new FracIndexError("cannot increment any more");
  if (i < b) return i;
  return ia + midpoint(fa, null);
}
