/**
 * A path on this site only: no `//host`, no backslash, no control
 * characters (browsers drop tabs and newlines, so `/\t/host` is `//host`).
 * The server's twin is `form::safe_return`.
 */
export function safeReturn(value: string): boolean {
  // eslint-disable-next-line no-control-regex
  return value.startsWith("/") && !value.startsWith("//") && !/[\u0000-\u001f\u007f\\]/.test(value);
}
