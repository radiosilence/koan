// The decode thread. `prepare` fetches and probes a track and leaves the
// worker free; `run` then blocks it until the session ends, so the page stops
// a running session through the shared stop flag rather than by message.
// `free` releases the session once the page has closed its audio context.

import init, { Loader, worker_init } from "./koan_web.js";

let ready = null;
let session = null;
let loading = false;

function start() {
  ready ??= (async () => {
    const module = await WebAssembly.compileStreaming(fetch(new URL("koan_web_bg.wasm", import.meta.url)));
    const wasm = await init({ module_or_path: module });
    worker_init();
    return { module, memory: wasm.memory };
  })();
  return ready;
}

async function load(track) {
  const res = await fetch(`/ui/stream/${encodeURIComponent(track)}`);
  if (!res.ok) {
    throw new Error(res.status === 401 ? "signed out" : `the server answered ${res.status}`);
  }
  const length = Number(res.headers.get("Content-Length"));
  if (!length) throw new Error("the server sent no Content-Length");
  const loader = new Loader(length);
  try {
    const reader = res.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      loader.push(value);
    }
  } catch (e) {
    loader.free();
    throw e;
  }
  return loader.finish();
}

const handlers = {
  async prepare({ track }) {
    if (session || loading) throw new Error("a track is already loaded");
    loading = true;
    let module, memory;
    try {
      ({ module, memory } = await start());
      session = await load(track);
    } finally {
      loading = false;
    }
    postMessage({
      type: "prepared",
      module,
      memory,
      format: {
        sampleRate: session.sample_rate,
        channels: session.channels,
        codec: session.codec,
        bitDepth: session.bit_depth,
        durationMs: session.duration_ms,
      },
      consumer: session.consumer,
      scratch: session.scratch,
      scratchLen: session.scratch_len,
      stop: session.stop,
    });
  },
  run() {
    if (!session) throw new Error("nothing is loaded");
    session.run();
    postMessage({ type: "done" });
  },
  free() {
    session?.free();
    session = null;
    postMessage({ type: "freed" });
  },
};

onmessage = async ({ data }) => {
  try {
    await handlers[data.type](data);
  } catch (e) {
    postMessage({ type: "error", during: data.type, message: String(e?.message ?? e) });
  }
};
