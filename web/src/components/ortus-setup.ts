import { LitElement, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, createOwner } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { MARK, controls, page } from "../ui.js";

/**
 * The first start: create the first owner with the token from the setup
 * link the server printed (it is in the address, after #).
 */
@customElement("ortus-setup")
export class OrtusSetup extends LitElement {
  static override styles = [page, controls];
  private readonly localize = new LocalizeController(this);
  @state() private error: MessageKey | undefined;
  @state() private busy = false;

  private readonly onHash = () => this.requestUpdate();

  override connectedCallback(): void {
    super.connectedCallback();
    addEventListener("hashchange", this.onHash);
  }

  override disconnectedCallback(): void {
    removeEventListener("hashchange", this.onHash);
    super.disconnectedCallback();
  }

  private get token(): string {
    return location.hash.slice(1);
  }

  private async submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const form = new FormData(event.target as HTMLFormElement);
    this.busy = true;
    this.error = undefined;
    try {
      await createOwner(this.token, String(form.get("name") ?? ""), String(form.get("email") ?? ""));
      history.replaceState(null, "", "/");
      location.assign("/");
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      this.error = code === "bad_setup_token" ? "setup.badLink" : code === "conflict" ? "setup.done" : code === "bad_request" ? "setup.check" : "setup.failed";
    } finally {
      this.busy = false;
    }
  }

  override render() {
    const missing = this.token.length === 0;
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "setup.title")}</h1>
        <p class="lead">${t(this.localize, missing ? "setup.noLink" : "setup.lead")}</p>
        ${missing
          ? nothing
          : html`<form @submit=${this.submit}>
              <label for="name">${t(this.localize, "setup.name")}</label>
              <input id="name" name="name" autocomplete="name" required maxlength="200" />
              <label for="email">${t(this.localize, "setup.email")}</label>
              <input id="email" name="email" type="email" autocomplete="email" required />
              ${this.error ? html`<p class="error" role="alert">${t(this.localize, this.error)}</p>` : nothing}
              <div class="actions"><button class="button primary" ?disabled=${this.busy}>${t(this.localize, "setup.create")}</button></div>
            </form>`}
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-setup": OrtusSetup;
  }
}
