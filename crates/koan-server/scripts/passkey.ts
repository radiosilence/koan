// Security keys and passkeys: a passkey signing in on its own from the
// sign-in page, a key answering a sign-in's second step, and registering one
// from the Account page. The server sends WebAuthn's options with their
// binary fields as base64url (koan_auth::webauthn) and reads the answers back
// the same way. Listeners sit on the document, so forms patched in later work
// too.

import * as z from "zod/mini";

const fromB64 = (text: string) => {
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  return Uint8Array.from(atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4)), (c) => c.charCodeAt(0));
};
const toB64 = (buf: ArrayBuffer) =>
  btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");

// The options as the server writes them: `creation_options`,
// `request_options` and `passkey_options` in koan_auth::webauthn. Parsed, not
// trusted: what the browser is handed is what these say.
const Descriptor = z.object({ type: z.literal("public-key"), id: z.string() });
const UserVerification = z.enum(["required", "preferred", "discouraged"]);
const RequestJson = z.object({
  rpId: z.string(),
  challenge: z.string(),
  timeout: z.number(),
  allowCredentials: z.array(Descriptor),
  userVerification: UserVerification,
});
const CreationJson = z.object({
  rp: z.object({ id: z.string(), name: z.string() }),
  user: z.object({ id: z.string(), name: z.string(), displayName: z.string() }),
  challenge: z.string(),
  pubKeyCredParams: z.array(z.object({ type: z.literal("public-key"), alg: z.number() })),
  timeout: z.number(),
  excludeCredentials: z.array(Descriptor),
  authenticatorSelection: z.object({
    residentKey: z.enum(["required", "preferred", "discouraged"]),
    userVerification: UserVerification,
  }),
  attestation: z.enum(["none", "indirect", "direct", "enterprise"]),
});
const RequestChallenge = z.object({ ticket: z.string(), options: RequestJson });
const CreationChallenge = z.object({ ticket: z.string(), options: CreationJson });
const Refusal = z.object({ message: z.string() });
type Descriptor = z.infer<typeof Descriptor>;
type RequestJson = z.infer<typeof RequestJson>;
type CreationJson = z.infer<typeof CreationJson>;

const descriptors = (list: Descriptor[]) => list.map((d) => ({ type: d.type, id: fromB64(d.id) }));

const requestOptions = (o: RequestJson) => ({
  rpId: o.rpId,
  challenge: fromB64(o.challenge),
  timeout: o.timeout,
  allowCredentials: descriptors(o.allowCredentials),
  userVerification: o.userVerification,
});

const creationOptions = (o: CreationJson) => ({
  rp: o.rp,
  user: { id: fromB64(o.user.id), name: o.user.name, displayName: o.user.displayName },
  challenge: fromB64(o.challenge),
  pubKeyCredParams: o.pubKeyCredParams,
  timeout: o.timeout,
  excludeCredentials: descriptors(o.excludeCredentials),
  authenticatorSelection: o.authenticatorSelection,
  attestation: o.attestation,
});

// A browser that cannot do WebAuthn, or a person who closed its prompt, says
// so in words a person reads.
function failure(e: unknown) {
  if (e instanceof DOMException && e.name === "NotAllowedError") return "Cancelled, or the key took too long.";
  if (e instanceof DOMException && e.name === "InvalidStateError") return "That key is already registered.";
  if (typeof PublicKeyCredential === "undefined") return "This browser cannot use security keys.";
  return e instanceof Error ? e.message : String(e);
}

const aborted = (e: unknown) => e instanceof DOMException && e.name === "AbortError";

function say(form: HTMLElement, message: string) {
  const next = form.nextElementSibling;
  const out =
    form.querySelector<HTMLElement>("[data-passkey-error]") ??
    (next instanceof HTMLElement && "passkeyError" in next.dataset ? next : null);
  if (!out) return;
  out.textContent = message;
  out.hidden = !message;
}

function input(form: HTMLFormElement, name: string) {
  const field = form.elements.namedItem(name);
  if (!(field instanceof HTMLInputElement)) throw new Error(`The form has no ${name}.`);
  return field;
}

async function send(url: string, body: unknown) {
  const res = await fetch(url, {
    method: "POST",
    // The Account page's routes take only the page's own posts, which say
    // so with Datastar's header.
    headers: { "content-type": "application/json", "datastar-request": "true" },
    body: JSON.stringify(body),
  });
  if (res.ok) return res;
  const said = res.headers.get("content-type")?.startsWith("application/json")
    ? Refusal.safeParse(await res.json()).data?.message
    : await res.text();
  throw new Error(said || "That did not work.");
}

// The prompt the page opened on its own, put away when a button asks
// outright: a browser runs one WebAuthn request at a time.
let offered: AbortController | null = null;

// Answer the form's challenge and send the answer. `conditional` offers the
// passkeys in the username field's suggestions rather than in a prompt. The
// sign-in page's form asks the server for its challenge first; the second
// step's carries it.
async function signIn(form: HTMLFormElement, conditional = false) {
  const asks = form.dataset["challenge"];
  let options: RequestJson;
  if (asks) {
    const asked = RequestChallenge.parse(await (await send(asks, {})).json());
    input(form, "ticket").value = asked.ticket;
    options = asked.options;
  } else {
    options = RequestJson.parse(JSON.parse(form.dataset["options"] ?? "{}"));
  }
  offered?.abort();
  offered = conditional ? new AbortController() : null;
  const request: CredentialRequestOptions = { publicKey: requestOptions(options) };
  if (offered) {
    request.mediation = "conditional";
    request.signal = offered.signal;
  }
  const credential = await navigator.credentials.get(request);
  if (!(credential instanceof PublicKeyCredential)) throw new Error("No key answered.");
  const response = credential.response;
  if (!(response instanceof AuthenticatorAssertionResponse)) throw new Error("The key's answer could not be read.");
  input(form, "assertion").value = JSON.stringify({
    id: toB64(credential.rawId),
    clientDataJson: toB64(response.clientDataJSON),
    authenticatorData: toB64(response.authenticatorData),
    signature: toB64(response.signature),
    userHandle: response.userHandle ? toB64(response.userHandle) : null,
  });
  form.submit();
}

// On the sign-in page, the browser's saved passkeys are offered at once,
// beside the saved passwords, where the browser can. Quietly: nobody asked
// for this one, so a failure (no passkeys, a busy server) says nothing, and
// the button is still there to ask outright.
const login = document.querySelector<HTMLFormElement>("form[data-passkey-login]");
if (login && typeof PublicKeyCredential !== "undefined" && "isConditionalMediationAvailable" in PublicKeyCredential) {
  PublicKeyCredential.isConditionalMediationAvailable().then((available) => {
    if (available) {
      signIn(login, true).catch(() => {});
    }
  });
}

async function register(form: HTMLFormElement) {
  const begin = form.dataset["begin"];
  if (!begin) throw new Error("The form names nowhere to begin.");
  const name = input(form, "securitykey").value.trim();
  const { ticket, options } = CreationChallenge.parse(await (await send(begin, { name })).json());
  const credential = await navigator.credentials.create({ publicKey: creationOptions(options) });
  if (!(credential instanceof PublicKeyCredential)) throw new Error("No key answered.");
  const response = credential.response;
  if (!(response instanceof AuthenticatorAttestationResponse)) throw new Error("The key's answer could not be read.");
  // The form sends what the key made through Datastar, whose answer draws the
  // section again, with the key listed and, for the first factor, the
  // recovery codes to keep.
  form.dispatchEvent(new CustomEvent("koan-key", {
    detail: {
      ticket,
      name,
      registration: {
        clientDataJson: toB64(response.clientDataJSON),
        attestationObject: toB64(response.attestationObject),
      },
    },
  }));
}

document.addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
    const run =
      "passkeySignIn" in form.dataset || "passkeyLogin" in form.dataset
        ? (f: HTMLFormElement) => signIn(f)
        : "passkeyRegister" in form.dataset
          ? register
          : null;
    if (!run) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const button = form.querySelector("button");
    if (button) button.disabled = true;
    say(form, "");
    run(form)
      .catch((e) => {
        if (!aborted(e)) say(form, failure(e));
      })
      .finally(() => {
        if (button) button.disabled = false;
      });
  },
  true,
);
