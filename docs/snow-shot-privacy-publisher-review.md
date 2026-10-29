# Privacy policy publisher review

Reviewed against the application and backend source on October 19, 2026.

These notes are for the publisher. The English and Simplified Chinese policies
are written for users; this appendix stays in the app repository and is not part
of the website. The operator and contact were supplied by the publisher:
Chao Huang, chao@mgchao.top.

The website sources are `D:/snow-apps-site/docs/en/privacy.md` and
`D:/snow-apps-site/docs/zh/privacy.md`. They build to `/privacy.html` and
`/zh/privacy.html`. The app's default privacy link is
`https://snowshot.top/privacy.html`, with the website's normal language selection.

## Source evidence reviewed

The review covered local storage, capture and recording, online features,
diagnostics, and MCP in the app, together with the backend in `E:/SnowShotApi`.
The website review covered browser preferences and external font loading.
The backend was read only; no secrets, production logs, or databases were accessed.

- `RequestContextFactory.cs` and `Identity.cs` derive a persistent identifier from
  the source IP. This is pseudonymous information, not anonymous information.
- `Entities.cs` stores operational/accounting metadata without content-history
  columns. `ImageMultipartReader.cs` uses cleared pooled buffers; recognition
  workers run private inference. Nginx request buffering and infrastructure logs
  can have independent storage, so this is not a universal no-storage commitment.
- `OpenAiChatClient.cs` and `OpenAiTranslationClient.cs` forward content to
  configurable providers. Their class names do not identify OpenAI as the
  processor. The runbook documents Alibaba Cloud Model Studio.
- `Options.cs` defaults to 90 days for settled operations and related events and
  attempts, 400 days for usage summaries, and 400 days for eligible inactive
  identities. The 90-day period starts at settlement. Unfinished operations and
  records needed by other operations are protected from cleanup. These are source
  defaults, not verified production settings or maximum retention promises.
- Worker logs contain IDs, timings, runtime details, and failure categories.
  API metrics/traces may export to OTLP. The checked-in nginx configuration does
  not establish access-log retention or disable upload buffering.
- `configurationstore.cpp` and `configurationarchive.cpp` save credentials in
  unencrypted files and regular settings exports. Damaged-settings backups can
  contain the same information; their cleanup threshold is 30 days.
- `capturehistoryrepository.cpp` and `pinnedwindowrepository.cpp` apply the history
  limits. Disabling history does not delete existing records and can pause
  cleanup. Clipboard images have a separate seven-day cleanup threshold.
- Recording source and recovery files can remain beside outputs and are outside
  capture-history cleanup. Editable recording sources retain mouse positions,
  clicks, and timing even when mouse effects are hidden. Keyboard key events are
  retained when the keyboard overlay is enabled. See `deferred.rs` and
  `screenrecordingcontroller.cpp`.
- Crashpad uploads are disabled. Diagnostic archives and crash dumps may contain
  sensitive content despite redaction. MCP starts disabled, authenticates local
  clients, and redacts credential fields in its settings export.
- The website remembers language and appearance in browser storage. Optional
  language diagnostics stay in the browser session. `theme/index.css` loads
  Google Fonts, so the public policy also describes that external request.

## Facts to confirm before publication

The source review supports the descriptions in the public policy. It cannot
establish where the live service runs or which account-level provider terms apply.
Record the following answers with the production configuration date. Keep the
public wording aligned with these facts as the service changes.

| Item | Confirmation needed |
| --- | --- |
| Hosting | Countries/regions and subprocessors for API, private workers, database, Redis, and any telemetry collector. |
| AI providers | Actual upstream accounts/endpoints, applicable product plans, privacy terms, and any cross-region routing. |
| Retention | Deployed operation/aggregate/identity settings, sweeper operation, and treatment of unfinished or linked records. |
| Logs and buffers | Reverse-proxy access/error logs, systemd/container logs, request buffering files, traces, security/incident records, and their cleanup. |
| Backups | Database/configuration backup locations, access controls, expiry, and treatment of deletion requests. |
| Training | Whether the operator reuses submitted content and the contractual terms applying to each upstream product plan. |
| Deletion requests | How records are located for an email request, verification, applicable exceptions, response process, and backup expiry. |
| Public page | Approved text and revision date, working English/Chinese URLs, and a monitored privacy mailbox. |

Alibaba Cloud's published privacy notice describes provider data handling, but
terms vary by site and product plan. Its international retention policy also
distinguishes standard retention, eligible zero-retention calls, and exceptions.
Do not describe the integration as universally zero-retention. Consult the applicable
[privacy notice](https://help.aliyun.com/zh/model-studio/privacy-notice) and, if the
international service applies, its
[data retention policy](https://www.alibabacloud.com/help/en/model-studio/data-retention-policy).
Do not transfer a zero-retention or no-training promise between different plans.

## Publication and Store submission

1. Confirm the outstanding deployment and provider facts above.
2. Keep both policy languages in sync. The website copies include page metadata;
   their policy text should match the copies in this repository.
3. Build and publish the website using the existing `D:/snow-apps-site` workflow.
   Integrating content into that checkout and building it locally does not make
   the page public. Website publication is separate from app packaging.
4. Open both public HTTPS pages without signing in and check their text and
   revision date, not just the HTTP status. Enter the policy URL in Partner Center
   and verify that the app's About link reaches the page.
5. Describe live generative AI functionality in listing/submission metadata and
   identify the existing in-app feedback route for reporting inappropriate output.
   Verify the mailbox and issue-reporting route are monitored.

See [Microsoft Store policies](https://learn.microsoft.com/en-us/windows/apps/publish/store-policies).
The published v7.20 has an effective date of October 22, 2026; check the version
applicable when submitting.
