import { LitElement, css, html, nothing, svg } from "lit";
import { customElement, property } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { LOCALES, setLocale, t } from "../i18n.js";
import { APP_ICONS, tileTints } from "../icons.js";
import type { MessageKey } from "../locales/en.js";
import { MARK } from "../ui.js";

const APPS = ["mail", "calendar", "drive", "docs", "chat", "calls", "tasks", "approvals"] as const;
const BENEFITS: [MessageKey, MessageKey][] = [
  ["site.b1Title", "site.b1"],
  ["site.b2Title", "site.b2"],
  ["site.b3Title", "site.b3"],
  ["site.b4Title", "site.b4"],
];
const STEPS: [MessageKey, MessageKey][] = [
  ["site.s1Title", "site.s1"],
  ["site.s2Title", "site.s2"],
  ["site.s3Title", "site.s3"],
];
const FAQ: [MessageKey, MessageKey][] = [
  ["site.q1", "site.a1"],
  ["site.q2", "site.a2"],
  ["site.q3", "site.a3"],
  ["site.q4", "site.a4"],
  ["site.q5", "site.a5"],
  ["site.q6", "site.a6"],
];
const SOURCE = "https://github.com/Ortus-Sovereign-Cloud";

/**
 * The project website, shown at the root to visitors of the public demo
 * (the `site.website` setting). Signed-in people get the app instead.
 */
@customElement("ortus-website")
export class OrtusWebsite extends LitElement {
  static override styles = [
    tileTints,
    css`
      :host {
        display: block;
        background: var(--ortus-bg);
        color: var(--ortus-text);
        --curve: cubic-bezier(0.22, 1, 0.36, 1);
      }
      a:focus-visible,
      button:focus-visible,
      select:focus-visible {
        outline: 2px solid var(--ortus-accent);
        outline-offset: 2px;
      }
      .skip {
        position: absolute;
        inset-inline-start: 16px;
        inset-block-start: -64px;
        z-index: 20;
        padding: 8px 12px;
        border-radius: 8px;
        background: var(--ortus-accent);
        color: var(--ortus-bg);
        font-weight: 600;
      }
      .skip:focus {
        inset-block-start: 16px;
      }
      nav {
        position: sticky;
        inset-block-start: 16px;
        z-index: 10;
        display: flex;
        align-items: center;
        gap: 24px;
        inline-size: fit-content;
        max-inline-size: calc(100% - 32px);
        margin: 16px auto 0;
        padding: 8px 8px 8px 16px;
        border-radius: 16px;
        border: 1px solid var(--ortus-line-strong);
        background: rgba(17, 19, 26, 0.72);
        backdrop-filter: blur(16px);
        box-sizing: border-box;
      }
      .brand {
        display: flex;
        align-items: center;
        gap: 8px;
        color: var(--ortus-text);
        font-weight: 600;
        text-decoration: none;
      }
      .brand svg {
        inline-size: 24px;
        block-size: 24px;
      }
      .links {
        display: flex;
        gap: 16px;
      }
      .links a {
        color: var(--ortus-text-2);
        font-size: 14px;
        text-decoration: none;
        transition: color 300ms var(--curve);
      }
      .links a:hover {
        color: var(--ortus-text);
      }
      select {
        block-size: 32px;
        border-radius: 8px;
        border: 1px solid var(--ortus-line-strong);
        background: var(--ortus-surface);
        color: var(--ortus-text);
        font: inherit;
        font-size: 14px;
      }
      .button {
        display: inline-flex;
        align-items: center;
        padding: 8px 12px;
        border-radius: 12px;
        border: 1px solid var(--ortus-line-strong);
        color: var(--ortus-text);
        font-size: 16px;
        line-height: 24px;
        font-weight: 600;
        text-decoration: none;
        transition: transform 300ms var(--curve), background-color 300ms var(--curve);
      }
      .button:hover {
        background: var(--ortus-hover);
      }
      .button:active {
        transform: scale(0.98);
      }
      .button.small {
        font-size: 14px;
        line-height: 20px;
      }
      .button.primary {
        border-color: transparent;
        background: var(--ortus-accent);
        color: var(--ortus-bg);
      }
      main {
        max-inline-size: 1120px;
        margin: 0 auto;
        padding: 0 16px;
      }
      section {
        padding-block: 96px 0;
      }
      .hero {
        padding-block-start: 96px;
        text-align: center;
      }
      h1 {
        max-inline-size: 680px;
        margin: 0 auto;
        font-size: 48px;
        line-height: 1;
        font-weight: 600;
        text-wrap: balance;
        background: linear-gradient(90deg, #ffffff, #9b9b9b);
        -webkit-background-clip: text;
        background-clip: text;
        color: transparent;
      }
      :host([dir="rtl"]) h1 {
        background: linear-gradient(270deg, #ffffff, #9b9b9b);
        -webkit-background-clip: text;
        background-clip: text;
      }
      .lead {
        max-inline-size: 680px;
        margin: 24px auto 0;
        color: var(--ortus-text-2);
        font-size: 18px;
        line-height: 28px;
        text-wrap: pretty;
      }
      .cta {
        display: flex;
        justify-content: center;
        gap: 12px;
        margin-block-start: 32px;
      }
      .proof {
        margin-block-start: 16px;
        color: var(--ortus-text-3);
        font-size: 14px;
        line-height: 20px;
      }
      .tiles {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 16px;
        max-inline-size: 680px;
        margin: 64px auto 0;
        padding: 24px;
        border-radius: 24px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .app {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 8px;
        padding: 16px 8px;
        border-radius: 16px;
        background: var(--ortus-surface-2);
        font-size: 14px;
        line-height: 20px;
        color: var(--ortus-text-2);
      }
      .app .tile {
        display: flex;
        align-items: center;
        justify-content: center;
        inline-size: 48px;
        block-size: 48px;
        border-radius: 12px;
      }
      .tagline p {
        max-inline-size: 680px;
        margin: 0 auto;
        font-size: 48px;
        line-height: 1;
        font-weight: 600;
        text-align: center;
        text-wrap: balance;
      }
      .word {
        color: rgba(236, 238, 243, 0.3);
        transition: color 600ms var(--curve);
      }
      .word.on {
        color: var(--ortus-text);
      }
      h2 {
        margin: 0 0 32px;
        font-size: 36px;
        line-height: 40px;
        font-weight: 600;
        text-wrap: balance;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
        gap: 16px;
      }
      .card {
        padding: 24px;
        border-radius: 16px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .card h3 {
        margin: 0 0 8px;
        font-size: 18px;
        line-height: 28px;
        font-weight: 600;
      }
      .card p,
      dd {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 16px;
        line-height: 24px;
        text-wrap: pretty;
      }
      .number {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        inline-size: 32px;
        block-size: 32px;
        margin-block-end: 16px;
        border-radius: 8px;
        background: var(--ortus-hover);
        color: var(--ortus-accent);
        font-weight: 600;
      }
      pre {
        margin: 12px 0 0;
        padding: 12px;
        border-radius: 8px;
        background: var(--ortus-bg);
        color: var(--ortus-text-2);
        font-family: "Geist Mono", ui-monospace, monospace;
        font-size: 14px;
        line-height: 20px;
        overflow-x: auto;
        direction: ltr;
        text-align: start;
      }
      dl {
        display: grid;
        gap: 16px;
        margin: 0;
      }
      dt {
        margin-block-end: 8px;
        font-size: 18px;
        line-height: 28px;
        font-weight: 600;
      }
      .final {
        padding-block-end: 96px;
        text-align: center;
      }
      footer {
        display: flex;
        flex-wrap: wrap;
        justify-content: center;
        gap: 24px;
        padding: 32px 16px;
        border-block-start: 1px solid var(--ortus-line);
        color: var(--ortus-text-3);
        font-size: 14px;
      }
      footer a {
        color: var(--ortus-text-2);
      }
      .reveal {
        opacity: 0;
        transform: translateY(32px);
        transition: opacity 700ms var(--curve), transform 700ms var(--curve);
      }
      .reveal.shown {
        opacity: 1;
        transform: none;
      }
      @media (prefers-reduced-motion: reduce) {
        .reveal {
          transform: none;
        }
      }
      @media (max-width: 720px) {
        .links {
          display: none;
        }
        nav {
          gap: 12px;
        }
        h1,
        .tagline p {
          font-size: 36px;
        }
        h2 {
          font-size: 30px;
          line-height: 36px;
        }
        .tiles {
          grid-template-columns: repeat(2, minmax(0, 1fr));
        }
        .cta {
          flex-direction: column;
          align-items: center;
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ type: Boolean }) signupOpen = false;
  private observer: IntersectionObserver | undefined;

  override updated(): void {
    this.setAttribute("dir", document.documentElement.dir || "ltr");
    this.observer?.disconnect();
    this.observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const target = entry.target as HTMLElement;
          if (target.classList.contains("words")) {
            // Each word turns on in reading order.
            target.querySelectorAll(".word").forEach((word, i) => setTimeout(() => word.classList.add("on"), i * 90));
          } else {
            target.classList.add("shown");
          }
          this.observer?.unobserve(target);
        }
      },
      { rootMargin: "0px 0px -15% 0px" },
    );
    this.renderRoot.querySelectorAll(".reveal, .words").forEach((element) => this.observer?.observe(element));
  }

  override disconnectedCallback(): void {
    this.observer?.disconnect();
    super.disconnectedCallback();
  }

  private primary() {
    return this.signupOpen
      ? html`<a class="button primary" href="/sign-up">${t(this.localize, "site.tryDemo")}</a>`
      : html`<a class="button primary" href="/sign-in">${t(this.localize, "site.signIn")}</a>`;
  }

  override render() {
    const lang = this.localize.lang.split("-")[0];
    const tagline = t(this.localize, "site.tagline");
    return html`
      <a class="skip" href="#content">${t(this.localize, "shell.skip")}</a>
      <nav aria-label="Ortus">
        <a class="brand" href="/">${unsafeSVG(MARK)}Ortus</a>
        <div class="links">
          <a href="#inside">${t(this.localize, "site.navInside")}</a>
          <a href="#install">${t(this.localize, "site.navInstall")}</a>
          <a href="#faq">${t(this.localize, "site.navFaq")}</a>
        </div>
        <select aria-label=${t(this.localize, "language.label")} @change=${(e: Event) => setLocale((e.target as HTMLSelectElement).value)}>
          ${LOCALES.map((l) => html`<option value=${l.code} ?selected=${l.code === lang} lang=${l.code}>${l.name}</option>`)}
        </select>
        ${this.signupOpen ? html`<a class="button small" href="/sign-in">${t(this.localize, "site.signIn")}</a>` : nothing}
      </nav>
      <main id="content">
        <section class="hero">
          <h1>${t(this.localize, "site.headline")}</h1>
          <p class="lead">${t(this.localize, "site.lead")}</p>
          <div class="cta">${this.primary()}</div>
          <p class="proof">${t(this.localize, "site.proof")}</p>
          <div class="tiles" role="list" aria-label=${t(this.localize, "site.navInside")}>
            ${APPS.map((id) => {
              const icon = APP_ICONS[id];
              return html`<div class="app" role="listitem">
                <span class="tile" data-app=${id} aria-hidden="true">${icon ? svg`<svg width="26" height="26" viewBox="0 0 48 48">${unsafeSVG(icon.svg)}</svg>` : nothing}</span>
                ${t(this.localize, `app.${id}` as MessageKey)}
              </div>`;
            })}
          </div>
        </section>
        <section class="tagline" aria-label=${tagline}>
          <p class="words" aria-hidden="true">${tagline.split(" ").map((w) => html`<span class="word">${w}</span> `)}</p>
        </section>
        <section id="inside" class="reveal">
          <h2>${t(this.localize, "site.insideTitle")}</h2>
          <div class="grid">
            ${BENEFITS.map(([title, text]) => html`<div class="card"><h3>${t(this.localize, title)}</h3><p>${t(this.localize, text)}</p></div>`)}
          </div>
        </section>
        <section id="install" class="reveal">
          <h2>${t(this.localize, "site.howTitle")}</h2>
          <div class="grid">
            ${STEPS.map(
              ([title, text], i) => html`<div class="card">
                <span class="number">${i + 1}</span>
                <h3>${t(this.localize, title)}</h3>
                <p>${t(this.localize, text)}</p>
                ${i === 0
                  ? html`<pre>sudo install/install.sh ./ortus</pre>
                      <pre>helm install ortus charts/ortus</pre>`
                  : nothing}
              </div>`,
            )}
          </div>
        </section>
        <section id="faq" class="reveal">
          <h2>${t(this.localize, "site.faqTitle")}</h2>
          <dl>
            ${FAQ.map(([q, a]) => html`<div class="card"><dt>${t(this.localize, q)}</dt><dd>${t(this.localize, a)}</dd></div>`)}
          </dl>
        </section>
        <section class="final reveal">
          <h2>${t(this.localize, "site.finalTitle")}</h2>
          <div class="cta">${this.primary()}</div>
        </section>
      </main>
      <footer>
        <a href=${SOURCE}>${t(this.localize, "site.source")}</a>
        ${this.signupOpen ? html`<a href="/terms">${t(this.localize, "terms.title")}</a>` : nothing}
        <span>${t(this.localize, "site.licence")}</span>
      </footer>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-website": OrtusWebsite;
  }
}
