import { LitElement, css, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { site } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { page } from "../ui.js";

const SECTIONS: [MessageKey, MessageKey][] = [
  ["terms.demoTitle", "terms.demo"],
  ["terms.guaranteeTitle", "terms.guarantee"],
  ["terms.dataTitle", "terms.data"],
  ["terms.deletionTitle", "terms.deletion"],
];

/** The terms and privacy notice of the public demo. */
@customElement("ortus-terms")
export class OrtusTerms extends LitElement {
  static override styles = [
    page,
    css`
      .card {
        max-inline-size: 640px;
      }
      h2 {
        margin: 24px 0 6px;
        font-size: 17px;
        font-weight: 600;
      }
      p {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
        line-height: 1.55;
      }
      a {
        display: inline-block;
        margin-block-start: 24px;
        color: var(--ortus-accent);
      }
    `,
  ];
  private readonly localize = new LocalizeController(this);
  @state() private days = 30;

  override connectedCallback(): void {
    super.connectedCallback();
    void site().then((s) => (this.days = s.account_days ?? 30)).catch(() => undefined);
  }

  override render() {
    return html`
      <main class="card">
        <h1>${t(this.localize, "terms.title")}</h1>
        ${SECTIONS.map(([title, text]) => html`<h2>${t(this.localize, title)}</h2><p>${t(this.localize, text, { days: this.days })}</p>`)}
        <a href="/sign-up">${t(this.localize, "terms.back")}</a>
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-terms": OrtusTerms;
  }
}
