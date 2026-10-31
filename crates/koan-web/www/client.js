// One track at a time: Load fetches and probes it in the worker, Play opens
// an audio context at its rate and starts the decoder once the worklet is up,
// and Stop, or the end of the track, tears both down. The ring and scratch
// buffer are freed only once the context has closed and the decoder has
// returned, since until then the worklet or the decoder may still touch them.

const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
const $ = (id) => document.getElementById(id);

// idle → loading → loaded → starting → playing → closing → freeing → idle,
// or broken, from which only a reload recovers.
let state = "idle";
let prepared = null;
let ctx = null;
let decoding = false;

function set(next, text) {
  state = next;
  if (text !== undefined) $("status").textContent = text;
  $("load").querySelector("button").disabled = state !== "idle";
  $("play").disabled = state !== "loaded";
  $("stop").disabled = !["loaded", "starting", "playing"].includes(state);
}

function describe(f) {
  const depth = f.bitDepth ? `${f.bitDepth}-bit, ` : "";
  const layout = f.channels === 1 ? "mono" : "stereo";
  return `${f.codec}, ${f.sampleRate} Hz, ${depth}${layout}, ${Math.round(f.durationMs / 1000)} s`;
}

// Silence at once; the decoder sees the stop flag the next time it wakes,
// which can be a few seconds while the ring is full.
// At a natural end the ring has drained into the output, which still holds
// its own latency's worth: closing at once would clip it.
async function end(text, { drain = false } = {}) {
  if (!["loaded", "starting", "playing"].includes(state)) return;
  set("closing", text);
  Atomics.store(new Uint8Array(prepared.memory.buffer), prepared.stop, 1);
  if (ctx) {
    if (drain) {
      const held = Math.max(ctx.baseLatency + (ctx.outputLatency || 0), 0.2);
      await new Promise((r) => setTimeout(r, held * 1000));
    }
    await ctx.close().catch(() => {});
    ctx = null;
  }
  free();
}

function broken(text) {
  set("broken", `${text} Reload the page to try again.`);
}

function free() {
  if (state !== "closing" || decoding || ctx) return;
  set("freeing");
  worker.postMessage({ type: "free" });
}

async function start(context, channels, resumed) {
  await context.audioWorklet.addModule(new URL("worklet.js", import.meta.url));
  if (ctx !== context || state !== "starting") return;
  const { module, memory, consumer, scratch, scratchLen } = prepared;
  const node = new AudioWorkletNode(context, "koan-output", {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [channels],
    processorOptions: { module, memory, consumer, scratch, scratchLen, channels },
  });
  node.onprocessorerror = () => end("The output failed.");
  node.port.onmessage = () => {
    if (ctx !== context || state !== "starting") return;
    decoding = true;
    worker.postMessage({ type: "run" });
    set("playing", `Playing ${describe(prepared.format)}`);
  };
  node.connect(context.destination);
  await resumed;
}

worker.onmessage = ({ data }) => {
  switch (data.type) {
    case "prepared":
      prepared = data;
      set("loaded", `Loaded ${describe(data.format)}`);
      break;
    case "done":
      decoding = false;
      if (state === "playing") end("Finished.", { drain: true });
      else free();
      break;
    case "freed":
      prepared = null;
      set("idle");
      break;
    case "error":
      if (data.during === "prepare") {
        set("idle", `Could not load: ${data.message}`);
      } else if (data.during === "free") {
        broken(`Could not release the track: ${data.message}`);
      } else {
        if (data.during === "run") decoding = false;
        if (state === "closing") free();
        else end(`The decoder failed: ${data.message}`);
      }
      break;
  }
};

worker.onerror = (e) => broken(`The worker failed: ${e.message}.`);

$("load").addEventListener("submit", (e) => {
  e.preventDefault();
  if (state !== "idle") return;
  set("loading", "Loading…");
  worker.postMessage({ type: "prepare", track: new FormData(e.target).get("track") });
});

// The context is made and resumed inside the click, which is what lets it
// start: Safari refuses to resume one outside a gesture.
$("play").addEventListener("click", () => {
  if (state !== "loaded") return;
  const { sampleRate, channels } = prepared.format;
  try {
    ctx = new AudioContext({ sampleRate, latencyHint: "playback" });
  } catch (e) {
    end(`This browser cannot play ${sampleRate} Hz: ${e.message}`);
    return;
  }
  // The ring holds samples at the source's rate; any other would play them
  // at the wrong pitch.
  if (ctx.sampleRate !== sampleRate) {
    end(`The browser opened ${ctx.sampleRate} Hz for a ${sampleRate} Hz track.`);
    return;
  }
  const resumed = ctx.resume();
  set("starting", "Starting…");
  start(ctx, channels, resumed).catch((e) => end(`Could not start the output: ${e.message}`));
});

$("stop").addEventListener("click", () => end("Stopped."));

set("idle");
