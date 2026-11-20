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
      a.button {
        margin-block-start: 16px;
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @state() private failed: "expired" | "other" | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    const token = location.hash.slice(1);
    history.replaceState(null, "", location.pathname);
    void this.accept(token);
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
