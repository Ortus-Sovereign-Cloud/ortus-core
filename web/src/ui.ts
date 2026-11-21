// Shared styles for Ortus controls: buttons, fields and the centred card
// of the pages shown before sign-in. Sizes follow the design's density
// rules: 40px controls, 13px floor, one filled accent button per screen.
import { css } from "lit";

export const controls = css`
  /* Dropdowns: one chevron of ours, with room, in every browser (the
     browser's own sits against the edge on macOS); mirrored right to left. */
  select {
    appearance: none;
    -webkit-appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%238b93a3' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 12px center;
    background-size: 12px 12px;
    padding-inline-end: 34px;
    cursor: pointer;
  }
  select:dir(rtl) {
    background-position: left 12px center;
  }
  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    block-size: 40px;
    padding-inline: 16px;
    border-radius: 10px;
    border: 1px solid var(--ortus-line-strong);
    background: none;
    color: var(--ortus-text);
    font: inherit;
    font-size: 15px;
    font-weight: 600;
    text-decoration: none;
    cursor: pointer;
  }
  .button.primary {
    border-color: transparent;
    background: var(--ortus-accent);
    color: var(--ortus-bg);
  }
  .button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .button:focus-visible,
  input:focus-visible,
  a:focus-visible {
    outline: 2px solid var(--ortus-accent);
    outline-offset: 2px;
  }
  label {
    display: block;
    margin-block: 16px 6px;
    color: var(--ortus-text-2);
    font-size: 13px;
    font-weight: 500;
  }
  input {
    box-sizing: border-box;
    inline-size: 100%;
    block-size: 44px;
    padding-inline: 12px;
    border-radius: 10px;
    border: 1px solid var(--ortus-line-input);
    background: var(--ortus-surface-2);
    color: var(--ortus-text);
    font: inherit;
    font-size: 15px;
  }
  .error {
    margin-block: 12px 0;
    padding: 10px 12px;
    border-radius: 10px;
    background: rgba(255, 122, 110, 0.12);
    color: var(--ortus-red);
    font-size: 14px;
  }
`;

export const page = css`
  :host {
    display: flex;
    min-block-size: 100vh;
    align-items: center;
    justify-content: center;
    padding: 24px 16px;
    box-sizing: border-box;
    background: var(--ortus-bg);
    color: var(--ortus-text);
  }
  .card {
    inline-size: 100%;
    max-inline-size: 440px;
    padding: 32px;
    box-sizing: border-box;
    border-radius: 18px;
    border: 1px solid var(--ortus-line);
    background: var(--ortus-surface);
  }
  .mark {
    display: block;
    margin-block-end: 20px;
  }
  h1 {
    margin: 0 0 8px;
    font-size: 24px;
    font-weight: 600;
    line-height: 1.25;
  }
  .lead {
    margin: 0;
    color: var(--ortus-text-2);
    font-size: 15px;
    line-height: 1.5;
  }
  .actions {
    margin-block-start: 24px;
  }
  .actions .button {
    inline-size: 100%;
  }
`;

export const MARK = `<svg width="40" height="40" viewBox="0 0 48 48" aria-hidden="true"><path d="M22 30 L4 30 A18 18 0 0 1 22 12 Z" fill="#8EA2FF"/><path d="M26 30 L26 12 A18 18 0 0 1 44 30 Z" fill="#6E86FF"/><rect x="4" y="34" width="40" height="5" rx="2.5" fill="#4FD1C5"/></svg>`;
