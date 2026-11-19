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

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`/api/v1${path}`, { headers: { Accept: "application/json" } });
  if (!response.ok) {
    const body = (await response.json().catch(() => undefined)) as ApiErrorBody | undefined;
    throw new ApiError(response.status, body?.error.code ?? "unknown", body?.error.message ?? response.statusText, body?.error.request_id ?? "");
  }
  return (await response.json()) as T;
}

export async function installedApps(): Promise<InstalledApp[]> {
  return (await get<{ apps: InstalledApp[] }>("/apps")).apps;
}
