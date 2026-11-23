// The public API, as the web client uses it. There is no private API.

export interface InstalledApp {
  /** Stable id: "mail", "calendar", ... */
  id: string;
  /** Which group of the app switcher it belongs to. */
  group: "communicate" | "organise" | "create" | "organisation";
  /** Where the app lives in the web client. */
  path: string;
  /** Whether it has a card on the start page (today.js). */
  today?: boolean;
}

export interface ApiErrorBody {
  error: { code: string; message: string; request_id: string };
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly requestId: string,
  ) {
    super(message);
  }
}

/** Asks the person to confirm it is them (step-up); set by the shell. */
let stepUp: (() => Promise<void>) | null = null;
export function onStepUp(handler: () => Promise<void>): void {
  stepUp = handler;
}

/** What was sent, sent again as it was (a string stays text). */
function payloadOf(sent: string | undefined): unknown {
  if (sent === undefined) return undefined;
  try {
    return JSON.parse(sent) as unknown;
  } catch {
    return sent;
  }
}

export async function call<T>(method: string, path: string, body?: unknown, again = true): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  // Every change says it comes from the Ortus client; other sites cannot.
  if (method !== "GET") headers["X-Requested-With"] = "ortus";
  // A string goes as it is (CSV); anything else as JSON.
  if (body !== undefined) headers["Content-Type"] = typeof body === "string" ? "text/plain; charset=utf-8" : "application/json";
  const payload = body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body);
  const response = await fetch(`/api/v1${path}`, { method, headers, body: payload });
  if (!response.ok) {
    const body = (await response.json().catch(() => undefined)) as ApiErrorBody | undefined;
    // An action that needs a recent proof: confirm, then once more.
    if (response.status === 403 && body?.error.code === "step_up" && stepUp && again) {
      await stepUp();
      return call<T>(method, path, payloadOf(payload), false);
    }
    throw new ApiError(response.status, body?.error.code ?? "unknown", body?.error.message ?? response.statusText, body?.error.request_id ?? "");
  }
  return (await response.json()) as T;
}

async function get<T>(path: string): Promise<T> {
  return call<T>("GET", path);
}

export interface Me {
  id: string;
  email: string;
  name: string;
  role: "member" | "admin" | "owner";
  language: string | null;
  last_app: string | null;
  chooser_seen: boolean;
  expires_at: number | null;
}

/** The signed-in account, or null; never an error for visitors. */
export async function session(): Promise<Me | null> {
  return (await get<{ user: Me | null }>("/session")).user;
}

export async function me(): Promise<Me> {
  return get<Me>("/me");
}

export async function setupNeeded(): Promise<boolean> {
  return (await get<{ needed: boolean }>("/setup")).needed;
}

export async function createOwner(token: string, name: string, email: string): Promise<Me> {
  return call<Me>("POST", "/setup", { token, name, email });
}

export type PasswordStep = { next: "code"; ticket: string };

export async function signInWithPassword(email: string, password: string): Promise<PasswordStep> {
  return call<PasswordStep>("POST", "/session/password", { email, password });
}

export async function signInWithCode(ticket: string, code: string): Promise<Me> {
  return call<Me>("POST", "/session/code", { ticket, code });
}

export async function signInWithRecoveryCode(email: string, code: string): Promise<Me & { recovery_codes_left: number }> {
  return call("POST", "/session/recovery", { email, code });
}

export interface Security {
  password: boolean;
  totp: boolean;
  recovery_codes_left: number;
  passkeys: { id: string; name: string; created_at: number; last_used_at: number | null }[];
  session_methods: string;
}

export async function mySecurity(): Promise<Security> {
  return get<Security>("/me/security");
}

export async function setPassword(password: string, current?: string): Promise<void> {
  await call("PUT", "/me/password", { password, current });
}

export async function startAuthenticator(): Promise<{ secret: string; uri: string; qr_svg: string }> {
  return call("POST", "/me/totp");
}

export async function confirmAuthenticator(code: string): Promise<void> {
  await call("POST", "/me/totp/confirm", { code });
}

export async function removeAuthenticator(): Promise<void> {
  await call("DELETE", "/me/totp");
}

export async function newRecoveryKit(): Promise<string[]> {
  return (await call<{ codes: string[] }>("POST", "/me/recovery-kit")).codes;
}

export interface Site {
  website: boolean;
  signup_open: boolean;
  account_days: number | null;
  quota_mb: number | null;
}

export async function site(): Promise<Site> {
  return get<Site>("/site");
}

export async function signUp(email: string, name: string, language: string, acceptTerms: boolean): Promise<string> {
  return (await call<{ ticket: string }>("POST", "/signup", { email, name, language, accept_terms: acceptTerms })).ticket;
}

export async function verifySignUp(ticket: string, code: string): Promise<Me> {
  return call<Me>("POST", "/signup/verify", { ticket, code });
}

/** Records the app opened (or null for the app chooser). */
export async function setPlace(app: string | null): Promise<void> {
  await call("PUT", "/me/place", { app });
}

export interface Provider {
  id: string;
  name: string;
  start: string;
}

/** The outside providers, and whether passwords are allowed here. */
export async function signInProviders(): Promise<{ providers: Provider[]; password: boolean }> {
  const answer = await get<{ providers: Provider[]; password?: boolean }>("/session/providers");
  return { providers: answer.providers, password: answer.password !== false };
}

/** Where to go after signing in: the app used last, or the app chooser. */
export function homeFor(me: Me): string {
  return me.chooser_seen && me.last_app ? `/${me.last_app}` : "/";
}

export async function signOut(): Promise<void> {
  await call("POST", "/session/sign-out");
}

export async function installedApps(): Promise<InstalledApp[]> {
  return (await get<{ apps: InstalledApp[] }>("/apps")).apps;
}
