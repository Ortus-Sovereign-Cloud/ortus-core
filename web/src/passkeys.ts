// Passkeys in the browser: the server's options (base64url) to what
// navigator.credentials wants, and the answers back to base64url.
import { call } from "./api.js";

function toBytes(text: string): ArrayBuffer {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

function toText(buffer: ArrayBuffer | null): string {
  if (!buffer) return "";
  let binary = "";
  for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function passkeysAvailable(): boolean {
  return typeof window.PublicKeyCredential === "function" && !!navigator.credentials;
}

/** Makes a passkey for the signed-in person and adds it to the account. */
export async function addPasskey(name: string): Promise<void> {
  const options = await call<Record<string, any>>("POST", "/me/passkeys/options");
  const credential = (await navigator.credentials.create({
    publicKey: {
      ...options,
      challenge: toBytes(options.challenge),
      user: { ...options.user, id: toBytes(options.user.id) },
      excludeCredentials: (options.excludeCredentials ?? []).map((c: { id: string; type: "public-key" }) => ({ ...c, id: toBytes(c.id) })),
    } as PublicKeyCredentialCreationOptions,
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("cancelled");
  const response = credential.response as AuthenticatorAttestationResponse;
  await call("POST", "/me/passkeys", {
    name,
    clientDataJSON: toText(response.clientDataJSON),
    attestationObject: toText(response.attestationObject),
  });
}

export async function removePasskey(id: string): Promise<void> {
  await call("DELETE", `/me/passkeys/${encodeURIComponent(id)}`);
}

/** Confirms it is the signed-in person with one of their passkeys (step-up). */
export async function confirmWithPasskey(): Promise<void> {
  const options = await call<Record<string, any>>("POST", "/session/confirm/options");
  const credential = (await navigator.credentials.get({
    publicKey: { ...options, challenge: toBytes(options.challenge), allowCredentials: [] } as PublicKeyCredentialRequestOptions,
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("cancelled");
  const response = credential.response as AuthenticatorAssertionResponse;
  await call("POST", "/session/confirm", {
    passkey: {
      id: credential.id,
      clientDataJSON: toText(response.clientDataJSON),
      authenticatorData: toText(response.authenticatorData),
      signature: toText(response.signature),
    },
  });
}

/** Signs in with any passkey of this installation. */
export async function signInWithPasskey<T>(): Promise<T> {
  const options = await call<Record<string, any>>("POST", "/session/passkey/options");
  const credential = (await navigator.credentials.get({
    publicKey: { ...options, challenge: toBytes(options.challenge), allowCredentials: [] } as PublicKeyCredentialRequestOptions,
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("cancelled");
  const response = credential.response as AuthenticatorAssertionResponse;
  return call<T>("POST", "/session/passkey", {
    id: credential.id,
    clientDataJSON: toText(response.clientDataJSON),
    authenticatorData: toText(response.authenticatorData),
    signature: toText(response.signature),
    userHandle: toText(response.userHandle),
  });
}
