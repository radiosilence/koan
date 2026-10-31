// The render callback. The worker owns the ring and the scratch buffer; this
// instance only reads, and allocates nothing once constructed, since a
// worklet cannot wait on the allocator's lock.

import "./polyfill.js";
import { initSync, render } from "./koan_web.js";

class KoanOutput extends AudioWorkletProcessor {
  constructor({ processorOptions: o }) {
    super();
    initSync({ module: o.module, memory: o.memory });
    this.memory = o.memory;
    this.consumer = o.consumer;
    this.scratch = o.scratch;
    this.scratchLen = o.scratchLen;
    this.channels = o.channels;
    this.view = new Float32Array(o.memory.buffer, o.scratch, o.scratchLen);
    // Gaps: a quantum the ring could not fill, then sound again. The end of
    // the track runs short too, and is not one.
    this.started = false;
    this.short = false;
    this.gaps = 0;
    this.port.postMessage("ready");
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const frames = out[0].length;
    const channels = this.channels;
    const got = render(this.consumer, this.scratch, Math.min(frames * channels, this.scratchLen), channels) / channels;
    if (got > 0 && this.short) {
      this.gaps++;
      this.port.postMessage({ gaps: this.gaps });
    }
    if (got > 0) this.started = true;
    this.short = this.started && got < frames;
    const view = this.view;
    for (let c = 0; c < channels; c++) {
      const ch = out[c];
      for (let i = 0; i < got; i++) ch[i] = view[i * channels + c];
      ch.fill(0, got);
    }
    return true;
  }
}

registerProcessor("koan-output", KoanOutput);
