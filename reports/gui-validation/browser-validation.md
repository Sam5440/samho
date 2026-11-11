# Browser GUI validation

Date: 2026-10-07. Service: real Rust `samho serve` at `http://127.0.0.1:374`, using an isolated runtime data directory. All configured language models were **demo simulation providers**. These checks validate frontend/API integration, not real model reasoning quality.

## Checked scenarios

| Scenario | Observed result |
| --- | --- |
| Runtime startup | State, models, sessions, objects, skills, event history, and SSE connected to the real Rust service. |
| New task, automatic grading | Submitted with automatic grading, complexity 4, no tier/model override. Completed with `demo-balanced`, persisted session `381ec078-eb1a-4380-812e-7470bba447d8`, trace `6733aa85-34cc-42b4-adb9-72eb0c7d769c`. |
| Session reuse, manual tier | The same session received a second user message. Moving the complexity slider to 7 disabled automatic grading. Tier `reasoning` produced `demo-reasoning`, trace `4fa80570-d6a5-47de-995e-22c09f04d78e`. The persisted session contained 5 messages: system plus two user/assistant exchanges. |
| Fixed model override | A new task with automatic grading enabled and model `demo-frontier` completed with that model, despite inferred balanced complexity. Trace `8e7f745b-4148-4865-bcc6-b2ab4e21e2d6`. |
| Trace details | Expanded `model.route`; displayed sequence, timestamp, session/trace IDs, span and parent span, Object ID, profile, provider, rationale, selected model, and tier. Each tested text task produced 5 actual trace events. |
| History after restart | Rebuilt/restarted Rust service kept its isolated data. Reload and session selection restored the two-turn session; trace default filtering showed its 10 events. |
| Object inspector | Displayed 9 registered Objects with correct kinds and expandable metadata. |
| MCP and Skills | Tabs selected successfully. This fixture had no MCP servers or Skills; explicit empty states were shown. Live MCP connection and Skill-loading browser interactions were **not** validated by this fixture. |
| Simulation labels | Runtime banner, model choices, and new assistant message metadata clearly showed simulation. The final bundled JavaScript checks provider `demo`, without inferring provider type from user-customizable model IDs. |
| Narrow viewport | Emulated 560 × 900 CSS pixels. One-column composition, horizontal session navigation, and lower runtime inspector; document scroll width 560, no horizontal overflow. |

## Static and console verification

- `node --check web/app.js` passed.
- 47 HTML IDs were unique; all static `byId` references resolved. Opening/closing `option` counts matched.
- No `innerHTML`, `outerHTML`, `insertAdjacentHTML`, or inline event handlers.
- After the final rebuild and cache-bypassing page reload: 0 console errors, 0 console warnings; all 10 startup network requests returned HTTP 200.
- The first build requested a missing favicon (404). An offline SVG data favicon fixed it. Temporary SSE reset/refused messages during the deliberate service restart cleared after reload.
- Cancellation UI is implemented, but a browser cancellation race was not exercised: demo tasks completed within milliseconds. CLI cancellation tests are separate evidence.
- Session-switch safety and stale-session-response protection were reviewed in the JavaScript; active-task switching was not timed interactively against a slow provider.

## Saved evidence

- `desktop.png`: desktop screenshot with expanded route metadata; 1710 × 934 CSS viewport, PNG 2565 × 1401 pixels (device pixel ratio 1.5).
- `narrow.png`: final reloaded narrow layout, 560 × 900 CSS viewport and PNG pixels.
- `browser-api-evidence.json`: actual event records and persisted sessions for the three browser tasks.

The Chrome screenshot tool rejected workspace save paths under its configured roots. Its returned image data was saved through the authorized local shell instead. No stub API, npm install, external fonts, or model credentials were used.
