# Client–Server Protocol Survey

Current shape of all client↔server communication, gathered as input for the
rewrite's API spec. Reflects the tree as of 2026-08-20 (branch `primary`,
post genre-promotion merge). File references are to the current layout.

---

## 1. Topology

```
mm-tui ──(bincode, sync framing)──┐
                                  ├── unix socket ── Witch (mm binary)
mm-web ──(bincode, async framing)─┘
   ▲
   │ HTTP + WS (JSON)
browser (React SPA)
```

- **One true protocol**: the Witch's socket protocol, defined entirely in
  `mm-meta` (`protocol.rs`, `domain_queries.rs`, `wire.rs`, `witch_types.rs`).
  Both the TUI and mm-web speak it.
- **mm-web is a JSON translator** over that protocol (`crates/mm-web`). It is
  stateless except for three things:
  1. cached `WitchStatus` (updated from event pushes, served at `GET /status`)
  2. a local session-token cache (populated at login, used to validate WS
     connections without a Witch round-trip)
  3. **deploy staging orchestration** in `/tx/stage-deploy` — the only place
     mm-web contains business logic (see §8 and Addendum A3).
- **Framing**: length-prefixed bincode (`wire.rs`), sync (`std::io`) for the
  TUI client, async (tokio) for mm-web's `WitchClient`. Requests are
  multiplexed by `request_id` (stamped by the client layer; HTTP handlers
  pass 0 and let `WitchClient` re-stamp).

### Wire envelope

```rust
enum WireRequest {
    Authenticated { request_id, token: SessionToken, body: AuthenticatedBody },
    Unauthenticated { request_id, body: UnauthenticatedBody },
    NotifyDbReady { request_id },          // post-setup lifecycle signal
}
enum WireResponse {
    Authenticated { request_id, result: Result<AuthenticatedResponse, ProtocolError> },
    Unauthenticated { request_id, result: Result<UnauthenticatedResponse, ProtocolError> },
    Ack { request_id },                    // for NotifyDbReady
    /* + unsolicited Event push (WitchEvent) */
}
```

### Authenticated request areas

```rust
enum AuthenticatedBody {                enum AuthenticatedResponse {
    Query(QueryPayload),                    Query(QueryResponse),
    Transaction(TransactionPayload),        Transaction(TransactionResponse),
    Command(CommandPayload),                Command(CommandResponse),
}                                       }
```

Request/response coupling is enforced by the `ProtocolQuery` /
`ProtocolCommand` traits (associated `Response` type; client send methods are
generic, extraction panics on variant mismatch = protocol bug).

### Errors

```rust
enum ProtocolError { Transaction(TransactionError), InvalidSession, Unauthorized, NotReady, Internal(String) }
enum TransactionError { AlreadyActive, NoActiveTransaction, NotAcceptingMutations, Other(String) }
```

---

## 2. Auth & lifecycle

Two authorization levels, exact-match, fail-closed:
`FirstTimeSetup` (only `CompleteSetup` accepted) and `Authenticated`.

| Operation | Protocol | Web route |
|---|---|---|
| Check if setup needed | `UnauthenticatedBody::SetupQuery` → `SetupStatus { needs_setup, suggested_root }` | `POST /setup/check` |
| Complete first-time setup | `CompleteSetup { root, first_user: Option<(user, pass)> }` → `SetupComplete` | `POST /setup/complete` |
| Login | `Login { username, password }` → `AuthResponse::Token(SessionToken) \| Failed(String)` | `POST /auth/login` — sets `mm_session` HttpOnly cookie **and** returns base64 token in body |
| Logout | **none** | `POST /auth/logout` — clears cookie only; token is never revoked (Addendum A9) |
| DB ready (post-setup) | `WireRequest::NotifyDbReady` → `Ack` | n/a (internal lifecycle) |

Web auth per-request: bearer token or cookie (`BearerToken` extractor); WS
accepts cookie or `?token=<base64>` fallback, validated against mm-web's local
cache only.

---

## 3. Server-pushed events

```rust
enum WitchEvent { StatusChanged(WitchStatus) }   // the only variant
```

- Broadcast by the Witch after any meaningful run-loop state change.
- mm-web relays every event verbatim as JSON over `GET /ws` (WebSocket) and
  also folds it into its `/status` cache.
- There is **no granular event vocabulary**: clients receive the full
  `WitchStatus` snapshot and diff generation counters
  (`mutations_generation`, `computations_generation`, `error_generation`,
  `config_generation`) to detect what changed.

### `WitchStatus` (the one status object)

Startup state, work state (`Idle/Working/Done` + pending counts +
`pending_by_label` map), reasoning level, initial-scan flag, db queue depth,
active-transaction snapshot (`TransactionSnapshot`: label, decision count,
mutation count, decision keys + labels), `handled_decision_kinds`,
external-fetch active flag + `FetchProgress` (acoustid/mb/discogs per-source
counts + rps), cover-art `CoverArtProgress`, Deezer `DeezerProgress`,
`last_error`, and the four generation counters. TS types are generated via
`ts-rs` (`#[ts(export)]`) into `frontend/src/api/generated/`.

---

## 4. HTTP surface (mm-web routes)

| Route | Method | Maps to |
|---|---|---|
| `/setup/check`, `/setup/complete`, `/auth/login`, `/auth/logout` | POST | unauthenticated area |
| `/status` | GET | local cache of `WitchEvent::StatusChanged` |
| `/config` | GET | `QueryPayload::Config` |
| `/config-kdl` | GET | `QueryPayload::ConfigKdl` |
| `/dir-config?path=` | GET | `QueryPayload::GetDirConfig` |
| `/queries/{name}` | GET **and** POST | `QueryPayload::Domain(...)` — 47 domain queries, kebab-case route names (§5–§9) |
| `/tx/start` `/tx/add` `/tx/remove` `/tx/details` `/tx/confirm` `/tx/discard` | POST (details: GET) | `TransactionPayload` lifecycle |
| `/tx/approve-releases` `/tx/apply-va-overrides` `/tx/edit-genre-vocabulary` `/tx/promote-genres` `/tx/promote-genres-for-inodes` | POST | server-side batch staging (`TransactionPayload::Batch*`) |
| `/tx/stage-deploy` | POST | **orchestrated in mm-web**, not a protocol op |
| `/commands/queue-task` `/commands/shutdown` | POST | `CommandPayload` |
| `/actions/execute` | POST | `ProtocolBinding` passthrough — **no frontend callers** (Addendum A2) |
| `/ws` | GET | event push |
| `/build-info` | GET | build timestamp (unauthenticated) |

Domain-query parameter encoding is per-query and ad hoc (Addendum A6):
- **Unit queries** (22 of them): no params.
- **Query-string queries**: serde variant names as strings (`?zone=Corpus`,
  `?confidence=High`), bools as literal `"true"`, ints parsed.
- **JSON-body queries** (7): whole struct posted (`GetAudioFilesByInodes`,
  `GetInodeDetails`, `GetCurrentTagValues`, `GetRecordingBatchData`,
  `GetReleaseStagingData`, `GetFileTagValues`, `SearchWithConditions`).

Responses strip the `DomainQueryResult` enum tag and return the inner value.

---

## 5. Subsystem: files/tracks (inode metadata lookup)

All are domain queries. This area has heavy overlap (Addendum B1).

| Query (route) | Params | Response | Purpose | Clients |
|---|---|---|---|---|
| `GetAudioFilesByInodes` (`audio-files-by-inodes`) | `inodes: Vec<i64>`, `zone` | `Vec<AudioFile>` | tag-editor launch by inode list | TUI |
| `GetInodeDetails` (`inode-details`) | `inodes: Vec<i64>`, `zone` | `Vec<InodeDetail>` | composite file + audio + DB tags, wizard expansion | TUI |
| `GetCorpusTags` (`corpus-tags`) | `inode` | `Vec<(String, String)>` | tag editor fill-from-DB, single inode | TUI |
| `GetCurrentTagValues` (`current-tag-values`) | `Vec<(inode, field)>` | `Vec<Option<String>>` | resolve current values pre-mutation | TUI |
| `GetFileTagValues` (`file-tag-values`) | `inodes`, `zone` | `Vec<(i64, Vec<(String,String)>)>` | read tags **from disk** for a batch | TUI, web |
| `GetTagEditorFiles` (`tag-editor-files`) | `rel_path`, `mode: Directory\|SingleFile` | `(Vec<AudioFile>, usize)` | tag editor load (dir recursive / siblings + index) | TUI |
| `GetBulkTagAggregate` (`bulk-tag-aggregate`) | `rel_path` | `BulkTagAggregate` | server-side bulk tag aggregate for a directory | TUI, web |
| `GetMissingTagAudioFiles` (`missing-tag-audio-files`) | — | `Vec<AudioFile>` | inodes from MissingTag signals → files | TUI |

## 6. Subsystem: browsing & search

| Query (route) | Params | Response | Purpose | Clients |
|---|---|---|---|---|
| `GetDirectoryListing` (`directory-listing`) | `zone`, `parent: Option<String>` | `Vec<DirectoryListingEntry>` | immediate children: subdirs w/ counts + files w/ display metadata, no tags | TUI, web |
| `SearchCorpusFiles` (`search-corpus`) | `query`, `limit` (default 200) | `Vec<SearchResult>` | substring search over paths + tag values | TUI, web |
| `SearchWithConditions` (`search-with-conditions`) | `conditions: Vec<SearchConditionWire>`, `zone`, `limit` | `Vec<SearchResult>` | structured search → SQL | TUI |
| `GetAllAudioFilesWithTags` (`all-audio-files-with-tags`) | `zone`, `include_library` | `Vec<AudioFileWithTags>` | full dump for client-side tag search — superseded by `SearchWithConditions` (Addendum B2) | TUI |

## 7. Subsystem: corpus health & resolution loaders

Summary rollup:

| Query (route) | Response | Purpose | Clients |
|---|---|---|---|
| `GetInsights` (`insights`) | `InsightsData` | signal counts / health rollup; polled | TUI, web |

Per-condition loaders (all unit queries unless noted; each backs one
resolution modal/view, and its staged fix goes through the transaction API
under the matching `DecisionKey`):

| Query (route) | Response | DecisionKey it feeds | Clients |
|---|---|---|---|
| `GetOobSummary` (`oob-summary`) — `bucket?`, `offset`, `limit` (clamped ≤1000; the only paginated query) | `OobSummary` | `OobResolution { bucket }` | TUI, web |
| `GetMovedFiles` (`moved-files`) | `MovedFileSummary` | `MovedFile` | TUI, web |
| `GetMissingFileData` (`missing-file-data`) | `MissingFileModalData` | `MissingFile` | TUI, web |
| `GetMissingDirectoryData` (`missing-directory-data`) | `MissingDirectoryModalData` | `MissingDirectory` | TUI, web |
| `GetCorruptFileData` (`corrupt-file-data`) | `CorruptFileModalData` | `CorruptFile` | TUI, web |
| `GetArtistNeedsPluralData` (`artist-needs-plural-data`) | `ArtistNeedsPluralModalData` | `ArtistPluralNormalization` | TUI, web |
| `GetSubparDuplicateData` (`subpar-duplicate-data`) | `SubparDuplicateModalData` | `SubparDuplicate` | TUI, web |
| `GetDirectoryClusterData` (`directory-cluster-data`) | `DirectoryClusterModalData` | `DirectoryCluster { cluster_index }` | TUI, web |
| `GetReleaseOverlapData` (`release-overlap-data`) | `DirectoryClusterModalData` (same type — Addendum B3) | `DirectoryCluster` | TUI, web |
| `GetLosslessRemuxData` (`lossless-remux-data`) | `LosslessRemuxModalData` | `LosslessRemux` | TUI, web |
| `GetMissingAlbumSingleSignals` (`missing-album-single-signals`) | `Vec<MissingAlbumSingleSignalWire>` | `MissingAlbum { group_index }` | TUI, web |
| `GetDiscExtractionData` (`disc-extraction-data`) — `map_letters_to_numbers` | `DiscExtractionModalData` | `DiscExtraction { group_index }` | TUI, web |
| `GetTagCanonicityResolution` (`tag-canonicity-resolution`) — `tag_name`, `zone`, `filter_existing_canonicals` | `TagCanonicityResolutionData` | `TagCanonicity { tag_name, cluster_index }` | TUI, web |
| `GetCompoundSplitResolution` (`compound-split-resolution`) — `tag_name`, `zone`, `safe_only` | `CompoundSplitResolutionData` | `CompoundSplitSafe/Review { tag_name, cluster_index }` | TUI, web |

## 8. Subsystem: work (external matching, packing, review, genres)

### Progress & control
- Progress: all via `WitchStatus` (fetch/cover-art/Deezer progress structs).
- Triggers: `CommandPayload::QueueTask(BackgroundTask)` where

```rust
enum BackgroundTask { ExternalFetch, CoverArtFetch, DeezerArtFetch,
                      ReleasePacking, SchemaReconciliation, Vacuum }
```

### Review/browse queries

| Query (route) | Params | Response | Purpose | Clients |
|---|---|---|---|---|
| `GetExternalMatches` (`external-matches`) | — | `ExternalMatchesData` | confidence buckets, packing counts, untagged; polled | TUI, web |
| `GetAcoustidMatches` (`acoustid-matches`) | `confidence` | `Vec<AcoustidMatchEntry>` | browse AcoustID matches, joined w/ MB cache | TUI, web |
| `GetReleaseReview` (`release-review`) | `filter` | `ReleaseReviewData` | packed releases by review category, fully joined | TUI, web |
| `GetManualReviewData` (`manual-review-data`) | `kind: ReviewKind` | `ManualReviewData` | manual review groups → `ManualReview { group_index }` decisions | TUI, web |
| `GetRecordingBatchData` (`recording-batch-data`) | `recording_ids`, `preferred_locales` | `RecordingBatchResult` | batch MB recording summaries from cache | TUI |
| `GetReleaseStagingData` (`release-staging-data`) | `release_ids`, `recording_ids`, `inodes` | `ReleaseStagingData` | MB cache bundle + current tags for approval staging | TUI |
| `GetPackingDirs` (`packing-dirs`) | — | `PackingDirsData` | assigned paths + directory categories | TUI |
| `GetPackingKnots` (`packing-knots`) | — | `Vec<PackingKnotData>` | conflict tangles needing review | TUI, web |
| `GetPackingInodePaths` (`packing-inode-paths`) | — | `Vec<(i64, String)>` | inode→path map for knot browser | TUI |
| `GetPackingBrowserData` (`packing-browser-data`) | `category_prefix` | `PackingBrowserData` | packed releases by category | TUI |
| `GetUnsolvedPackingData` (`unsolved-packing-data`) | `category` | `Vec<(i64, String, UnmatchedCorpusTrackData)>` | unmatched tracks by unsolved category | TUI |
| `GetVaOverrideReview` (`va-override-review`) | — | `VaOverrideReview` | VA-override suggestions, enriched | TUI, web |

### Genre system

| Query (route) | Params | Response | Clients |
|---|---|---|---|
| `GetGenreVocabulary` (`genre-vocabulary`) | — | `GenreVocabulary` (canonicals, aliases, implications, ledger counts) | TUI, web |
| `GetUnresolvedGenreObservations` (`unresolved-genre-observations`) | — | `UnresolvedGenreObservations` | TUI, web |
| `GetGenrePromotionReview` (`genre-promotion-review`) | — | `GenrePromotionReview` (per-release chip strips) | TUI, web |
| `GetGenrePromotionInodeDetail` (`genre-promotion-inode-detail`) | `release_id` | `GenrePromotionInodeDetail` (lazy row expansion) | TUI, web |
| `GetGenreCoverageSummary` (`genre-coverage-summary`) | — | `GenreCoverageSummary` | TUI, web |

### Server-side batch staging ops (transaction area)

These stage decisions server-side from a compact intent payload — the shape
the whole write API should converge on (see memory: VA-overrides pattern).
None of them confirm; the operator reviews and hits `/tx/confirm`.

| Protocol op | Intent payload | Decision keying | Summary response |
|---|---|---|---|
| `BatchApproveReleases` | `release_ids: Vec<String>` | `MbReleaseApproval { release_id }` (idempotent per tx) | `ApprovalSummary` (staged/skipped releases + tracks) |
| `BatchApplyVaOverrides` | `Vec<VaOverrideApplication { release_id, albumartist }>` | `VaOverrideApplication { release_id }` | `VaOverrideSummary` |
| `BatchEditGenreVocabulary` | `Vec<GenreVocabularyOp>` | `GenreVocabularyEdit` (singleton; re-issue replaces) | `GenreVocabularySummary { staged_ops }` |
| `BatchPromoteGenres` | `Vec<GenrePromotionApplication>` (chip exclusions) | `GenrePromote { release_id }` (re-stage replaces) | `GenrePromotionStagingSummary` |
| `BatchPromoteGenresForInodes` | `inodes: Vec<i64>` (cap: `genre-write-back.bulk-cap`, default 5000) | `GenrePromote { release_id }` | `GenrePromotionStagingSummary` |

## 9. Subsystem: deploy

| Surface | Shape | Notes |
|---|---|---|
| `GetDeployStatus` (`deploy-status`) | unit → `DeployStatus` | library health + per-library counts; polled |
| `GetDeployData` (`deploy-data`) | unit → `DeployModalData` | counts only — deliberately cheap to open the view |
| `GetDeployReadyFiles` (`deploy-ready-files`) | unit → `Vec<DeploySignalFile>` | ~500k rows on a real corpus; fetched only at commit time; no pagination |
| `POST /tx/stage-deploy` | no body → `{staged, total_ops, skipped}` | mm-web-side orchestration: `GetDeployData` + `GetDeployReadyFiles` + `Config` → `PathResolver` → `prepare_deploy_decisions()` (from mm-ui) → `Discard` (silently drops any active tx!) → `Start` → N × `AddDecision`. Decisions under `Deploy` / `DeploySidecars` keys. |
| TUI deploy | same building blocks, in-process | mutations built in TUI action handlers via `ConfirmationGesture` |

## 10. Subsystem: config

| Surface | Shape | Notes |
|---|---|---|
| `QueryPayload::Config` / `GET /config` | → `Config` | parsed snapshot |
| `QueryPayload::ConfigKdl` / `GET /config-kdl` | → `String` | raw KDL, for source detection in the editor |
| `QueryPayload::GetDirConfig(path)` / `GET /dir-config?path=` | → `Option<SourceDir>` | per-source-dir config |
| Config writes | mutations via tx: `ApplyConfigEdits` (key `ConfigEdit`), `ApplyDirConfigEdit` / `ApplyBatchDirConfigEdits` (key `DirConfigEdit { source_path }`) | no hot-reload; container restart required (known) |

These three reads are bespoke `QueryPayload` variants + bespoke HTTP routes,
predating the domain-query system (Addendum B5).

## 11. Subsystem: mutations & transactions (the write path)

### Transaction lifecycle (protocol ops ↔ web routes 1:1)

`Start { label }` → `AddDecision { key, decision }` / `RemoveDecision { key }`
→ `GetDetails` → `Confirm` | `Discard`. Single active transaction; `Start`
while active → `AlreadyActive` (web: 409, frontend swallows it and appends).

- `Decision = { label, mutations: Vec<Mutation> }` — the serializable unit of
  operator intent. TUI-side it can only be minted by exchanging a
  `ConfirmationGesture`; **the web has no equivalent gate** — any authenticated
  `POST /tx/add` with hand-built mutation JSON is accepted (Addendum A1).
- `DecisionKey` (27 variants) keys staged decisions and drives idempotency/
  replacement semantics; `DecisionKey::allowed_mutation_kinds()` whitelists
  which `MutationKind`s each decision may carry (debug-assert on staging).
- `GetDetails` → `Vec<DecisionDetail { key, label, mutations }>` — full
  mutation payloads for the review diff.

### Mutation vocabulary (26 variants — the real write API)

| Group | Mutations |
|---|---|
| Tags | `ApplyTagOps`, `ApplyDbTagsToDisk`, `FlushTagsToDisk`, `AssimilateDiskTagsToDb` |
| Files/index | `IndexFileFromPath`, `Move`, `UpdateFilePath`, `DropFromIndex`, `DropDirectoryFromIndex`, `AcknowledgeMovedFiles`, `AcknowledgeMtimeOnly`, `ResolveOobBucket` |
| Zone/stash | `StashFromZone`, `StashLeftovers` |
| Transcode | `Transcode` |
| Deploy | `HardLink`, `LibraryMove` |
| Signal emission (expectation marking) | `EmitCanonicalTag`, `EmitExpectedOverlap`, `EmitExpectedDuplicate`, `EmitExpectedMissingTag` |
| External data | `DropExternalMatch` |
| Config | `ApplyConfigEdits`, `ApplyDirConfigEdit`, `ApplyBatchDirConfigEdits` |
| Genres | `EditGenreVocabulary` |

On the web wire these serialize as serde externally-tagged JSON
(`{"DropFromIndex": {"path": ..., "inode": ..., "zone": "corpus"}}`), and the
React views hand-construct them (`Resolve.tsx`) — the mutation schema is
duplicated by hand in TypeScript.

## 12. Client coverage matrix

- **TUI**: full surface (all 47 domain queries, all tx ops, commands).
- **Web**: 34/47 domain query routes + tx lifecycle + all batch endpoints
  except `promote-genres-for-inodes` + `queue-task`. Not reachable from the
  web UI: tag editor loaders (`tag-editor-files`, `corpus-tags`,
  `audio-files-by-inodes`, `inode-details`, `current-tag-values`,
  `missing-tag-audio-files`), packing browsers (`packing-dirs`,
  `packing-inode-paths`, `packing-browser-data`, `unsolved-packing-data`),
  structured search (`search-with-conditions`, `all-audio-files-with-tags`),
  MB staging internals (`recording-batch-data`, `release-staging-data`),
  `commands/shutdown`, `actions/execute`.

---

# Addendum: gaps, duplication, and warts

## A. Structural issues

**A1. Mutation-construction responsibility is split inconsistently.**
Three write styles coexist:
1. client-built mutations posted raw to `/tx/add` (web Resolve views, TUI
   everywhere) — the browser hand-mirrors the serde `Mutation` enum in TS;
2. server-side intent staging (`BatchApprove*`, `BatchApplyVaOverrides`,
   genre ops) — client sends ids/intent, server builds mutations;
3. mm-web-side orchestration (`/tx/stage-deploy`) — neither client nor Witch.
The rewrite should pick (2) universally: clients send intent + decision key,
the server owns mutation construction and path resolution. This also closes
the gap that the web bypasses the `ConfirmationGesture` witness discipline —
today any authenticated caller can stage arbitrary well-formed mutations.

**A2. `/actions/execute` is dead surface.** Zero frontend callers; its
`Transaction` arm is a hard error by design, `Navigation` is a server
round-trip no-op, and the `Command` arm duplicates `/commands/*`.
`ProtocolBinding` itself conflates three unrelated things (command, tx display
metadata, navigation) and shouldn't cross the wire in the rewrite.

**A3. `/tx/stage-deploy` lives in the wrong layer.** It's the only mm-web
endpoint with business logic (queries + config fetch + `PathResolver` +
decision building + N sequential `AddDecision` round-trips), it silently
`Discard`s any active transaction, and it duplicates what
`BatchApproveReleases` demonstrated the protocol can do server-side. Should be
a `TransactionPayload::StageDeploy` protocol op in the Witch. It also pulls
`GetDeployReadyFiles` (~500k rows) across the socket only to send derived
mutations right back.

**A4. Pagination exists exactly once.** `GetOobSummary` (offset/limit,
clamped). Everything else is unbounded: `GetDeployReadyFiles` (~500k rows),
`GetAllAudioFilesWithTags` (full corpus dump), moved-files, the modal loaders,
`GetGenrePromotionReview`. The rewrite needs a uniform pagination/cursor
convention, not per-endpoint retrofits.

**A5. Status/events are a single fat snapshot.** One event variant
(`StatusChanged`) carrying full `WitchStatus`; clients diff four generation
counters to guess what changed. Works, but: three transports for the same data
(protocol `Status` query, `GET /status` cache, WS push), no
subscription/filtering, and every state change re-ships `pending_by_label`
maps to every client. A rewrite event vocabulary (typed deltas or at least
per-area sequence numbers) would remove the counter-diffing idiom.

**A6. Parameter encoding on `/queries/{name}` is ad hoc.** GET and POST both
accepted on every query; serde variant names as query-string values
(`?zone=Corpus`), bools as the literal string `"true"` (anything else =
false, silently), 7 queries require a JSON body instead, and unit queries
ignore all input. Pick one transport shape per query in the spec (probably:
everything POST + JSON body, or a real GET convention with typed params).

**A7. Error surface is inconsistent.** Queries/tx map to HTTP error codes;
commands return `200 {"ok": false, "error": ...}`; `TransactionError` is four
variants with `Other(String)` carrying most real semantics; the HTTP shim
hand-copies each summary struct field-by-field into `json!` blocks (drift-prone,
duplicated between `tx_to_json` and the per-endpoint matches); and
`DiscardSummary` is an empty struct that gets flattened to
`{"discarded": true}` — dead weight on the wire.

**A8. UI concepts leak into protocol types.** Response types are named after
TUI modals (`MissingFileModalData`, `DiscExtractionModalData`,
`LosslessRemuxModalData`, `TagEditorLoadMode`), and comments in
`domain_queries.rs` still organize queries by migration "waves" rather than
domain. The rewrite spec should name resources by domain, not by the widget
that first consumed them.

**A9. No session lifecycle.** No logout/revocation/expiry anywhere in the
protocol; web logout only clears the cookie; mm-web's local token cache grows
forever and survives token invalidation on the Witch side (there isn't any).
Rewrite needs: token expiry, a revoke op, and mm-web cache eviction.

## B. Collapsible duplicates

**B1. Inode metadata lookup is 8 overlapping queries** (§5). At minimum:
one batch `inodes → details` endpoint with a field-selection/level parameter
(paths | audio | db-tags | disk-tags) replaces `GetAudioFilesByInodes`,
`GetInodeDetails`, `GetCorpusTags`, `GetCurrentTagValues`, `GetFileTagValues`;
`GetTagEditorFiles`/`GetBulkTagAggregate` are path-scoped variants of the same
thing (`rel_path → file set` + optional aggregate).

**B2. Search is triplicated.** `SearchCorpusFiles` (substring),
`SearchWithConditions` (structured), `GetAllAudioFilesWithTags` (dump for
client-side evaluation — explicitly superseded by `SearchWithConditions` per
its own doc comment, still shipped). One search endpoint where the free-text
form is sugar for a condition.

**B3. `GetDirectoryClusterData` vs `GetReleaseOverlapData`**: two routes,
identical response type (`DirectoryClusterModalData`). One endpoint with a
cluster-kind discriminator.

**B4. Two-phase promote endpoints.** `BatchPromoteGenres` (explicit
applications) vs `BatchPromoteGenresForInodes` (server fans out). Same
builder, same keying, same summary — a single op with an
`applications | inodes` intent union.

**B5. Config reads sit outside the domain-query system** (bespoke
`QueryPayload` variants + bespoke routes) for no remaining reason; fold
`Config`/`ConfigKdl`/`GetDirConfig` into the same query registry as everything
else in the rewrite.

**B6. Health modal loaders are 14 near-identical unit queries.** Each returns
"the full set of condition X" with no filtering or paging. They could be one
`GET /signals/{kind}` family with a shared envelope (count + bounded page +
per-kind payload), which would also give the rewrite paging (A4) for free.

## C. Smaller notes

- `request_id` is part of the HTTP-visible types but meaningless there
  (always 0; re-stamped by `WitchClient`). Keep it wire-internal in the spec.
- `NotifyDbReady`/`Ack` is a lifecycle special case in the wire enum used only
  during first-time setup; fold into the setup exchange.
- `GetOobSummary.limit` is clamped server-side (good — the only place input
  bounding happens at all).
- Web `409` on `/tx/start` is used as a control-flow signal by the frontend
  (append-to-existing-tx); the spec should make "open or join transaction"
  explicit instead.
- `BackgroundTask::ReleasePacking` always rescores from scratch; there is no
  incremental variant at the protocol level.
- TS types come from two sources: `ts-rs` generated (`generated/types.ts`) for
  data types, but hand-written for mutation payloads and request bodies —
  half the wire contract is unchecked (see A1).
