// Security keys and passkeys: a passkey signing in on its own from the
// sign-in page, a key answering a sign-in's second step, and registering one
// from the Account page. The server sends WebAuthn's options
// with their binary fields as base64url and reads the answers back the same
// way. Listeners sit on the document, so forms patched in later work too.

const fromB64 = (text: string) => {
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  return Uint8Array.from(atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4)), (c) => c.charCodeAt(0));
};
const toB64 = (buf: ArrayBuffer) =>
  btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");

type Descriptor = { type: "public-key"; id: string };
type Options = Record<string, unknown> & {
  challenge: string;
  user?: { id: string; name: string; displayName: string };
  allowCredentials?: Descriptor[];
  excludeCredentials?: Descriptor[];
};

const descriptors = (list: Descriptor[] = []) => list.map((d) => ({ type: d.type, id: fromB64(d.id) }));

// A browser that cannot do WebAuthn, or a person who closed its prompt, says
// so in words a person reads.
function failure(e: unknown) {
  if (e instanceof DOMException && e.name === "NotAllowedError") return "Cancelled, or the key took too long.";
  if (e instanceof DOMException && e.name === "InvalidStateError") return "That key is already registered.";
  if (!window.PublicKeyCredential) return "This browser cannot use security keys.";
  return e instanceof Error ? e.message : String(e);
}

function say(form: HTMLElement, message: string) {
  const next = form.nextElementSibling;
  const out =
    form.querySelector<HTMLElement>("[data-passkey-error]") ??
    (next instanceof HTMLElement && "passkeyError" in next.dataset ? next : null);
  if (!out) return;
  out.textContent = message;
  out.hidden = !message;
}

// The prompt the page opened on its own, put away when a button asks
// outright: a browser runs one WebAuthn request at a time.
let offered: AbortController | null = null;

// Answer the form's challenge and send the answer. `conditional` offers the
// passkeys in the username field's suggestions rather than in a prompt.
async function signIn(form: HTMLFormElement, conditional = false) {
  const options: Options = JSON.parse(form.dataset["options"] ?? "{}");
  offered?.abort();
  offered = conditional ? new AbortController() : null;
  const credential = (await navigator.credentials.get({
    publicKey: {
      ...options,
      challenge: fromB64(options.challenge),
      allowCredentials: descriptors(options.allowCredentials),
    } as PublicKeyCredentialRequestOptions,
    ...(offered ? { mediation: "conditional" as CredentialMediationRequirement, signal: offered.signal } : {}),
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("No key answered.");
  const response = credential.response as AuthenticatorAssertionResponse;
  form.querySelector<HTMLInputElement>("input[name=assertion]")!.value = JSON.stringify({
    id: toB64(credential.rawId),
    clientDataJson: toB64(response.clientDataJSON),
    authenticatorData: toB64(response.authenticatorData),
    signature: toB64(response.signature),
    userHandle: response.userHandle ? toB64(response.userHandle) : null,
  });
  form.submit();
}

// On the sign-in page, the browser's saved passkeys are offered at once,
// beside the saved passwords, where the browser can.
const login = document.querySelector<HTMLFormElement>("form[data-passkey-login]");
if (login && window.PublicKeyCredential?.isConditionalMediationAvailable) {
  PublicKeyCredential.isConditionalMediationAvailable().then((available) => {
    if (available) {
      signIn(login, true).catch((e) => {
        if (!(e instanceof DOMException && e.name === "AbortError")) say(login, failure(e));
      });
    }
  });
}

async function post(url: string, body: unknown) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (res.headers.get("content-type")?.startsWith("application/json")) {
    const json = await res.json();
    if (!res.ok) throw new Error(json.message ?? "That did not work.");
    return json;
  }
  if (!res.ok) throw new Error((await res.text()) || "That did not work.");
  return res.text();
}

async function register(form: HTMLFormElement) {
  const name = form.querySelector<HTMLInputElement>("input[name=securitykey]")!.value.trim();
  const { ticket, options } = (await post(form.dataset["begin"]!, { name })) as { ticket: string; options: Options };
  const credential = (await navigator.credentials.create({
    publicKey: {
      ...options,
      challenge: fromB64(options.challenge),
      user: { ...options.user!, id: fromB64(options.user!.id) },
      excludeCredentials: descriptors(options.excludeCredentials),
    } as PublicKeyCredentialCreationOptions,
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("No key answered.");
  const response = credential.response as AuthenticatorAttestationResponse;
  const html = (await post(form.action, {
    ticket,
    name,
    registration: {
      clientDataJson: toB64(response.clientDataJSON),
      attestationObject: toB64(response.attestationObject),
    },
  })) as string;
  // The section again, with the key listed and, for the first factor, the
  // recovery codes to keep.
  document.getElementById("second-factor")!.outerHTML = html;
}

document.addEventListener("submit", (event) => {
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
  const button = form.querySelector<HTMLButtonElement>("button");
  if (button) button.disabled = true;
  say(form, "");
  run(form)
    .catch((e) => {
      if (!(e instanceof DOMException && e.name === "AbortError")) say(form, failure(e));
    })
    .finally(() => {
      if (button) button.disabled = false;
    });
}, true);
