// AudioWorkletGlobalScope has no TextDecoder or TextEncoder, which
// wasm-bindgen's glue constructs when it loads. The worklet passes only
// numbers across, so these never see real text; ASCII is enough.
globalThis.TextDecoder ??= class {
  decode(bytes) {
    let text = "";
    for (const b of bytes ?? []) text += String.fromCharCode(b);
    return text;
  }
};
globalThis.TextEncoder ??= class {
  encode(text = "") {
    return Uint8Array.from(text, (c) => c.charCodeAt(0) & 0x7f);
  }
};
