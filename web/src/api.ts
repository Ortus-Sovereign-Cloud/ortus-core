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

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
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

export async function me(): Promise<Me> {
  return get<Me>("/me");
}

export async function setupNeeded(): Promise<boolean> {
  return (await get<{ needed: boolean }>("/setup")).needed;
}

export async function createOwner(token: string, name: string, email: string): Promise<Me> {
  return call<Me>("POST", "/setup", { token, name, email });
}

export async function signOut(): Promise<void> {
  await call("POST", "/session/sign-out");
}

export async function installedApps(): Promise<InstalledApp[]> {
  return (await get<{ apps: InstalledApp[] }>("/apps")).apps;
}
