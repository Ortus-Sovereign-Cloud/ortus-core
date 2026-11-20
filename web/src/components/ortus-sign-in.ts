import { LitElement, html } from "lit";
import { customElement } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { t } from "../i18n.js";
import { MARK, controls, page } from "../ui.js";

/** The sign-in page. Its ways of signing in arrive with passkeys and passwords. */
@customElement("ortus-sign-in")
export class OrtusSignIn extends LitElement {
  static override styles = [page, controls];
  private readonly localize = new LocalizeController(this);

  override render() {
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "signIn.title")}</h1>
        <p class="lead">${t(this.localize, "signIn.lead")}</p>
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-sign-in": OrtusSignIn;
  }
}
