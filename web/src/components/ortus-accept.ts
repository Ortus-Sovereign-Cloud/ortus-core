import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, call } from "../api.js";
import { t } from "../i18n.js";
import { MARK, controls, page } from "../ui.js";

/**
 * The page an invitation link opens (`/welcome/accept#token`): it signs
 * the person in and leads to their first day. The token travels in the
 * fragment, so it never reaches a server log.
 */
@customElement("ortus-accept")
export class OrtusAccept extends LitElement {
  static override styles = [
    page,
    controls,
    css`
      a.button,
      button.button {
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
  @state() private failed: "expired" | "other" | undefined;
  /** On a phone, the link waits: the app or this browser takes it. */
  @state() private choosing = false;
  private token = "";

  override connectedCallback(): void {
    super.connectedCallback();
    this.token = location.hash.slice(1);
    history.replaceState(null, "", location.pathname);
    if (this.token && OrtusAccept.onPhone()) {
      this.choosing = true;
      return;
    }
    void this.accept(this.token);
  }

  /** Whether this browser runs on a phone or tablet (where the app may be). */
  static onPhone(): boolean {
    const data = (navigator as Navigator & { userAgentData?: { mobile?: boolean } }).userAgentData;
    return data?.mobile === true || /iPhone|iPad|iPod|Android/.test(navigator.userAgent);
  }

  /** The app's own link: it joins this installation with the code. */
  private appLink(): string {
    return `ortus://join?server=${encodeURIComponent(location.origin)}&code=${encodeURIComponent(this.token)}`;
  }

  private async accept(token: string): Promise<void> {
    if (!token) {
      this.failed = "expired";
      return;
    }
    try {
      await call("POST", "/invitation/accept", { token });
      location.assign("/welcome");
    } catch (error) {
      this.failed = error instanceof ApiError && error.code === "invitation_expired" ? "expired" : "other";
    }
  }

  override render() {
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "accept.title")}</h1>
        ${this.failed
          ? html`<p class="error" role="alert">${t(this.localize, this.failed === "expired" ? "accept.expired" : "accept.failed")}</p>
              <a class="button" href="/sign-in">${t(this.localize, "accept.signIn")}</a>`
          : this.choosing
            ? html`<p class="lead">${t(this.localize, "accept.onPhone")}</p>
                <a class="button primary" href=${this.appLink()}>${t(this.localize, "accept.openApp")}</a>
                <button
                  class="button"
                  @click=${() => {
                    this.choosing = false;
                    void this.accept(this.token);
                  }}
                >
                  ${t(this.localize, "accept.continueHere")}
                </button>
                <p class="hint">${t(this.localize, "accept.noApp")}</p>`
            : html`<p class="lead" role="status">${t(this.localize, "accept.working")}</p>`}
        ${nothing}
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-accept": OrtusAccept;
  }
}
