import { LitElement, css, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call } from "../api.js";
import { t } from "../i18n.js";
import { safeReturn } from "../safe-return.js";
import { MARK, controls, page } from "../ui.js";

/**
 * The phone's way into a web page (`/session/web#t=<ticket>&next=<path>`):
 * the app opened it in the system's browser view with a one-time ticket.
 * The page says whose ticket it is and signs the browser in only on the
 * person's yes (a link from someone else must not quietly sign a browser
 * in as them), then goes on to the path. The fragment never reaches a
 * server; the ticket is traded by POST.
 */
@customElement("ortus-web-ticket")
export class OrtusWebTicket extends LitElement {
  static override styles = [
    page,
    controls,
    css`
      .row {
        display: flex;
        gap: 12px;
        margin-block-start: 16px;
      }
      .row .button {
        flex: 1;
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  private ticket = "";
  private next = "/";
  @state() private whose: { name: string; email: string } | null = null;
  @state() private broken = false;
  @state() private busy = false;

  override connectedCallback(): void {
    super.connectedCallback();
    const params = new URLSearchParams(location.hash.slice(1));
    this.ticket = params.get("t") ?? "";
    const next = params.get("next") ?? "/";
    this.next = safeReturn(next) ? next : "/";
    // The ticket leaves the address (and the history) at once.
    history.replaceState(null, "", location.pathname);
    void this.look();
  }

  private async look(): Promise<void> {
    if (!/^[0-9a-f]{16,200}$/.test(this.ticket)) {
      this.broken = true;
      return;
    }
    try {
      this.whose = await call<{ name: string; email: string }>("POST", "/session/web-ticket/whose", { ticket: this.ticket });
    } catch {
      this.broken = true;
    }
  }

  private async go(): Promise<void> {
    this.busy = true;
    try {
      await call("POST", "/session/web-ticket", { ticket: this.ticket });
      location.assign(this.next);
    } catch {
      this.busy = false;
      this.broken = true;
    }
  }

  override render() {
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "webTicket.title")}</h1>
        ${this.broken
          ? html`<p class="error" role="alert">${t(this.localize, "webTicket.expired")}</p>
              <div class="row"><a class="button" href="/sign-in">${t(this.localize, "webTicket.signIn")}</a></div>`
          : this.whose
            ? html`<p class="lead">${t(this.localize, "webTicket.as", { name: this.whose.name, email: this.whose.email })}</p>
                <div class="row">
                  <a class="button" href="/sign-in">${t(this.localize, "webTicket.notMe")}</a>
                  <button class="button primary" ?disabled=${this.busy} @click=${this.go}>${t(this.localize, "webTicket.continue")}</button>
                </div>`
            : html`<p class="loading" role="status" aria-busy="true"></p>`}
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-web-ticket": OrtusWebTicket;
  }
}
