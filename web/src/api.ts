// The public API, as the web client uses it. There is no private API.

export interface InstalledApp {
  /** Stable id: "mail", "calendar", ... */
  id: string;
  /** Which group of the app switcher it belongs to. */
  group: "communicate" | "organise" | "create" | "organisation";
  /** Where the app lives in the web client. */
  path: string;
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

export async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  // Every change says it comes from the Ortus client; other sites cannot.
  if (method !== "GET") headers["X-Requested-With"] = "ortus";
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(`/api/v1${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (!response.ok) {
    const body = (await response.json().catch(() => undefined)) as ApiErrorBody | undefined;
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

export async function signInProviders(): Promise<Provider[]> {
  return (await get<{ providers: Provider[] }>("/session/providers")).providers;
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
