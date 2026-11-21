import { LitElement, css, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { t } from "../i18n.js";
import { MARK, controls, page } from "../ui.js";
import { OrtusAccept } from "./ortus-accept.js";

/** An installation's invitation, as an app link carries it after `#`. */
export interface Relayed {
  server: string;
  code: string;
}

/**
 * Reads `s=<installation>&c=<code>`: the installation as an https origin
 * (http only on this machine, for tests), the code as an invitation's.
 */
export function relayed(fragment: string): Relayed | undefined {
  const params = new URLSearchParams(fragment);
  const code = params.get("c") ?? "";
  let server: URL;
  try {
    server = new URL(params.get("s") ?? "");
  } catch {
    return undefined;
  }
  const local = ["localhost", "127.0.0.1"].includes(server.hostname);
  if (!(server.protocol === "https:" || (server.protocol === "http:" && local))) return undefined;
  if (!/^[A-Za-z0-9_-]{16,200}$/.test(code)) return undefined;
  return { server: server.origin, code };
}

/**
 * The page the shared app link opens (`/i#s=…&c=…`) when the Ortus app did
 * not take it: on a phone without the app, or on a computer. It names the
 * installation the invitation leads to and goes there only when asked, so
 * it never sends anyone somewhere unseen. The fragment never reaches a
 * server.
 */
@customElement("ortus-relay")
export class OrtusRelay extends LitElement {
  static override styles = [
    page,
    controls,
    css`
      a.button {
        display: block;
        box-sizing: border-box;
        inline-size: 100%;
        margin-block-start: 16px;
        text-align: center;
      }
      .hint {
        margin-block-start: 16px;
        color: var(--ortus-text-3);
        font-size: 13px;
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @state() private invitation: Relayed | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    this.invitation = relayed(location.hash.slice(1));
    history.replaceState(null, "", location.pathname);
  }

  override render() {
    const i = this.invitation;
    const host = i ? new URL(i.server).host : "";
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "accept.title")}</h1>
        ${i
          ? html`<p class="lead">${t(this.localize, "relay.lead", { host })}</p>
              ${OrtusAccept.onPhone()
                ? html`<a class="button primary" href=${`ortus://join?server=${encodeURIComponent(i.server)}&code=${encodeURIComponent(i.code)}`}>
                    ${t(this.localize, "accept.openApp")}
                  </a>`
                : ""}
              <a class="button ${OrtusAccept.onPhone() ? "" : "primary"}" href=${`${i.server}/welcome/accept#${i.code}`}>${t(this.localize, "relay.continue", { host })}</a>
              ${OrtusAccept.onPhone() ? html`<p class="hint">${t(this.localize, "accept.noApp")}</p>` : ""}`
          : html`<p class="error" role="alert">${t(this.localize, "relay.broken")}</p>`}
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-relay": OrtusRelay;
  }
}
