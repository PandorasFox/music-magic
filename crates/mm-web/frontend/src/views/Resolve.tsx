import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  useMissingFileData,
  useMissingDirectoryData,
  useCorruptFileData,
  useArtistNeedsPluralData,
  useSubparDuplicateData,
  useLosslessRemuxData,
  useMovedFiles,
  useOobSummary,
} from "../api/queries";
import { post, ApiError } from "../api/client";
import type {
  MissingFileModalData,
  MissingDirectoryModalData,
  CorruptFileModalData,
  ArtistNeedsPluralModalData,
  SubparDuplicateModalData,
  LosslessRemuxModalData,
  MovedFileInfo,
  OobFile,
  ConflictBucket,
} from "../api/generated/types";

// -- Shared staging helper --

async function stageBatch(
  key: unknown,
  label: string,
  mutations: unknown[],
  navigate: (path: string) => void,
  setError: (msg: string | null) => void,
  setStaging: (v: boolean) => void,
) {
  if (mutations.length === 0) return;
  setStaging(true);
  setError(null);
  try {
    try {
      await post("/tx/start", { label });
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 409)) throw e;
    }
    await post("/tx/add", {
      key,
      decision: { label, mutations },
    });
    navigate("/tx");
  } catch (err) {
    setError(err instanceof Error ? err.message : String(err));
  } finally {
    setStaging(false);
  }
}

// -- Missing Files --

function MissingFiles() {
  const { data, isLoading, error } = useMissingFileData();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;
  const d = data as MissingFileModalData;

  function dropAll() {
    const mutations = [
      ...d.restorable.map((f) => ({
        DropFromIndex: { path: f.corpus_path, inode: f.inode, zone: "corpus" },
      })),
      ...d.non_restorable.map((f) => ({
        DropFromIndex: { path: f.corpus_path, inode: f.inode, zone: "corpus" },
      })),
    ];
    void stageBatch("MissingFile", "Drop missing files", mutations, navigate, setErr, setStaging);
  }

  return (
    <div className="resolve-view">
      <h2>Missing Files ({d.restorable.length + d.non_restorable.length})</h2>
      {err && <div className="form-error">{err}</div>}
      {d.restorable.length > 0 && (
        <section className="resolve-section">
          <h3>Restorable ({d.restorable.length})</h3>
          {d.restorable.map((f) => (
            <div key={f.corpus_path} className="resolve-row">
              <span>{f.corpus_path}</span>
              <span className="resolve-muted">from {f.library_path}</span>
            </div>
          ))}
        </section>
      )}
      {d.non_restorable.length > 0 && (
        <section className="resolve-section">
          <h3>Non-restorable ({d.non_restorable.length})</h3>
          {d.non_restorable.map((f, i) => (
            <div key={i} className="resolve-row">
              <span>{f.corpus_path}</span>
            </div>
          ))}
        </section>
      )}
      <div className="resolve-actions">
        <button onClick={dropAll} disabled={staging}>
          {staging ? "Staging..." : "Drop All From Index"}
        </button>
      </div>
    </div>
  );
}

// -- Missing Directories --

function MissingDirectories() {
  const { data, isLoading, error } = useMissingDirectoryData();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;
  const d = data as MissingDirectoryModalData;

  function dropAll() {
    const mutations = d.directories.map((dir) => ({
      DropDirectoryFromIndex: { directory_path: dir },
    }));
    void stageBatch("MissingDirectory", "Drop missing directories", mutations, navigate, setErr, setStaging);
  }

  return (
    <div className="resolve-view">
      <h2>Missing Directories ({d.directories.length})</h2>
      {err && <div className="form-error">{err}</div>}
      {d.directories.map((dir) => (
        <div key={dir} className="resolve-row">{dir}</div>
      ))}
      <div className="resolve-actions">
        <button onClick={dropAll} disabled={staging}>
          {staging ? "Staging..." : "Drop All From Index"}
        </button>
      </div>
    </div>
  );
}

// -- Corrupt Files --

function CorruptFiles() {
  const { data, isLoading, error } = useCorruptFileData();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;
  const d = data as CorruptFileModalData;

  function stashAll() {
    const mutations = d.files.flatMap((f) => [
      { StashFromZone: { path: f.corpus_path, zone: "corpus", reason: "corrupt" } },
      { DropFromIndex: { path: f.corpus_path, inode: f.inode, zone: "corpus" } },
    ]);
    void stageBatch("CorruptFile", "Stash corrupt files", mutations, navigate, setErr, setStaging);
  }

  return (
    <div className="resolve-view">
      <h2>Corrupt Files ({d.files.length})</h2>
      {err && <div className="form-error">{err}</div>}
      {d.files.map((f) => (
        <div key={f.corpus_path} className="resolve-row">{f.corpus_path}</div>
      ))}
      <div className="resolve-actions">
        <button onClick={stashAll} disabled={staging}>
          {staging ? "Staging..." : "Stash All"}
        </button>
      </div>
    </div>
  );
}

// -- Subpar Duplicates --

function SubparDuplicates() {
  const { data, isLoading, error } = useSubparDuplicateData();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;
  const d = data as SubparDuplicateModalData;

  function stashAll() {
    const mutations = d.files.flatMap((f) => [
      { StashFromZone: { path: f.corpus_path, zone: "corpus", reason: "subpar" } },
      { DropFromIndex: { path: f.corpus_path, inode: f.inode, zone: "corpus" } },
    ]);
    void stageBatch("SubparDuplicate", "Stash subpar duplicates", mutations, navigate, setErr, setStaging);
  }

  return (
    <div className="resolve-view">
      <h2>Subpar Duplicates ({d.files.length})</h2>
      {err && <div className="form-error">{err}</div>}
      {d.files.map((f) => (
        <div key={f.corpus_path} className="resolve-row">
          <span>{f.corpus_path}</span>
          <span className="resolve-muted">{f.reason} (vs {f.superior_path}, {f.similarity_score.toFixed(1)}%)</span>
        </div>
      ))}
      <div className="resolve-actions">
        <button onClick={stashAll} disabled={staging}>
          {staging ? "Staging..." : "Stash All Subpar"}
        </button>
      </div>
    </div>
  );
}

// -- Lossless Remux --

function LosslessRemux() {
  const { data, isLoading, error } = useLosslessRemuxData();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;
  const d = data as LosslessRemuxModalData;

  function remuxAll() {
    const mutations = d.files.map((f) => ({
      Transcode: { corpus_path: f.corpus_path, inode: f.inode, target_format: "flac" },
    }));
    void stageBatch("LosslessRemux", "Remux to FLAC", mutations, navigate, setErr, setStaging);
  }

  return (
    <div className="resolve-view">
      <h2>Lossless Remux Candidates ({d.files.length})</h2>
      {err && <div className="form-error">{err}</div>}
      {Object.entries(d.file_counts).map(([ft, count]) => (
        <div key={ft} className="resolve-row">
          <span>{ft}</span>
          <span className="em-count">{count}</span>
        </div>
      ))}
      <div className="resolve-actions">
        <button onClick={remuxAll} disabled={staging}>
          {staging ? "Staging..." : "Remux All to FLAC"}
        </button>
      </div>
    </div>
  );
}

// -- Moved Files --

function MovedFiles() {
  const { data, isLoading, error } = useMovedFiles();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data || data.total === 0) return <div className="view-placeholder">No moved files</div>;

  function acceptAll() {
    // One bulk mutation regardless of scale — the worker applies the whole
    // outstanding set. Sending one per file meant ~660k mutations.
    const mutations = [{ AcknowledgeMovedFiles: { observed_count: data!.total } }];
    void stageBatch("MovedFile", "Accept moved files", mutations, navigate, setErr, setStaging);
  }

  const truncated = data.total > data.sample.length;

  return (
    <div className="resolve-view">
      <h2>Moved Files ({data.total})</h2>
      {err && <div className="form-error">{err}</div>}
      {truncated && (
        <div className="resolve-muted">
          Showing first {data.sample.length}; accepting applies to all {data.total}.
        </div>
      )}
      {data.sample.map((f: MovedFileInfo) => (
        <div key={f.inode} className="resolve-row">
          <span className="resolve-muted">{f.old_path}</span>
          <span className="deploy-stale-arrow">&rarr;</span>
          <span>{f.new_path}</span>
        </div>
      ))}
      <div className="resolve-actions">
        <button onClick={acceptAll} disabled={staging}>
          {staging ? "Staging..." : "Accept All Moves"}
        </button>
      </div>
    </div>
  );
}

// -- Artist Needs Plural --

function ArtistNeedsPlural() {
  const { data, isLoading, error } = useArtistNeedsPluralData();
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;
  const d = data as ArtistNeedsPluralModalData;

  function pluralizeAll() {
    const ops: Array<{ inode: number; tag_name: string; old_value: string | null; new_value: string | null }> = [];
    for (const f of d.files) {
      if (f.data.needs_artist && f.data.artist_values.length > 1) {
        for (const v of f.data.artist_values) {
          ops.push({ inode: f.inode, tag_name: "ARTIST", old_value: v, new_value: null });
        }
        ops.push({ inode: f.inode, tag_name: "ARTIST", old_value: null, new_value: f.data.artist_values.join("; ") });
        for (const v of f.data.artist_values) {
          ops.push({ inode: f.inode, tag_name: "ARTISTS", old_value: null, new_value: v });
        }
      }
      if (f.data.needs_album_artist && f.data.album_artist_values.length > 1) {
        for (const v of f.data.album_artist_values) {
          ops.push({ inode: f.inode, tag_name: "ALBUMARTIST", old_value: v, new_value: null });
        }
        ops.push({ inode: f.inode, tag_name: "ALBUMARTIST", old_value: null, new_value: f.data.album_artist_values.join("; ") });
        for (const v of f.data.album_artist_values) {
          ops.push({ inode: f.inode, tag_name: "ALBUMARTISTS", old_value: null, new_value: v });
        }
      }
    }
    if (ops.length === 0) return;
    const mutations = [{ ApplyTagOps: { ops, zone: "Corpus" } }];
    void stageBatch("ArtistPluralNormalization", "Pluralize artist tags", mutations, navigate, setErr, setStaging);
  }

  return (
    <div className="resolve-view">
      <h2>Artist Tags Need Pluralizing ({d.files.length})</h2>
      {err && <div className="form-error">{err}</div>}
      {d.files.map((f) => (
        <div key={f.corpus_path} className="resolve-row">
          <span>{f.corpus_path}</span>
          <span className="resolve-muted">
            {f.data.needs_artist && f.data.needs_album_artist
              ? "[ARTIST+ALBUMARTIST]"
              : f.data.needs_artist
                ? "[ARTIST]"
                : "[ALBUMARTIST]"}
          </span>
        </div>
      ))}
      <div className="resolve-actions">
        <button onClick={pluralizeAll} disabled={staging}>
          {staging ? "Staging..." : "Pluralize All"}
        </button>
      </div>
    </div>
  );
}

// -- OOB Tag Resolution --

function OobResolution({ bucket }: { bucket: ConflictBucket }) {
  const { data, isLoading, error } = useOobSummary(bucket);
  const navigate = useNavigate();
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{error.message}</div>;

  // Indexed access is `number | undefined` under noUncheckedIndexedAccess,
  // though BUCKET_INDEX only ever yields 0..3 into a fixed 4-tuple.
  const total = data?.counts[BUCKET_INDEX[bucket]] ?? 0;
  if (!data || total === 0)
    return <div className="view-placeholder">No OOB files in this bucket</div>;

  // One bulk mutation regardless of scale — the worker walks the bucket
  // itself. Sending one per file meant ~500k mutations during a tag rewrite.
  function resolveAll(action: string, label: string) {
    const mutations = [
      { ResolveOobBucket: { bucket, action, observed_count: total } },
    ];
    void stageBatch(
      { OobResolution: { bucket } },
      label,
      mutations,
      navigate,
      setErr,
      setStaging,
    );
  }

  const truncated = data.offset + data.files.length < total;

  return (
    <div className="resolve-view">
      <h2>OOB Files: {bucket} ({total})</h2>
      {err && <div className="form-error">{err}</div>}
      {truncated && (
        <div className="resolve-muted">
          Showing first {data.files.length}; actions apply to all {total}.
        </div>
      )}
      {data.files.map((f: OobFile) => (
        <div key={f.inode} className="resolve-row resolve-row--col">
          <span>{f.path}</span>
          {f.mismatches.length > 0 && (
            <div className="resolve-mismatches">
              {f.mismatches.map((m, i) => (
                <div key={i} className="resolve-mismatch">
                  <span className="resolve-mismatch__field">{m.field}</span>
                  <span className="resolve-muted">db: {m.db_value ?? "(none)"}</span>
                  <span className="resolve-muted">disk: {m.disk_value ?? "(none)"}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      ))}
      <div className="resolve-actions">
        {bucket === "MtimeOnly" ? (
          <button
            onClick={() => resolveAll("AcknowledgeMtime", "Acknowledge mtime changes")}
            disabled={staging}
          >
            {staging ? "Staging..." : `Acknowledge All (${total})`}
          </button>
        ) : (
          <>
            <button
              onClick={() => resolveAll("AssimilateDiskToDb", "Assimilate file tags \u2192 DB")}
              disabled={staging}
            >
              {staging ? "Staging..." : `Assimilate Files \u2192 DB (${total})`}
            </button>
            <button
              onClick={() => resolveAll("ApplyDbToDisk", "Apply DB tags \u2192 files")}
              disabled={staging}
            >
              {staging ? "Staging..." : `Apply DB \u2192 Files (${total})`}
            </button>
          </>
        )}
      </div>
    </div>
  );
}

// Position of each bucket within `OobSummary.counts`.
const BUCKET_INDEX: Record<ConflictBucket, number> = {
  MtimeOnly: 0,
  DbOnly: 1,
  DiskOnly: 2,
  Conflict: 3,
};

// -- Route dispatcher --

const RESOLUTION_TYPES: Record<string, string> = {
  "missing-files": "Missing Files",
  "missing-directories": "Missing Directories",
  "corrupt-files": "Corrupt Files",
  "subpar-duplicates": "Subpar Duplicates",
  "lossless-remux": "Lossless Remux",
  "moved-files": "Moved Files",
  "oob-mtime": "OOB: Mtime Only",
  "oob-db": "OOB: DB Only",
  "oob-disk": "OOB: Disk Only",
  "oob-conflict": "OOB: Conflicts",
  "artist-needs-plural": "Artist Plural Normalization",
};

export function Resolve() {
  const { type } = useParams<{ type: string }>();
  const navigate = useNavigate();

  return (
    <div>
      <button className="em-back" onClick={() => navigate("/")}>&larr; Health</button>
      {type === "missing-files" && <MissingFiles />}
      {type === "missing-directories" && <MissingDirectories />}
      {type === "corrupt-files" && <CorruptFiles />}
      {type === "subpar-duplicates" && <SubparDuplicates />}
      {type === "lossless-remux" && <LosslessRemux />}
      {type === "moved-files" && <MovedFiles />}
      {type === "oob-mtime" && <OobResolution bucket="MtimeOnly" />}
      {type === "oob-db" && <OobResolution bucket="DbOnly" />}
      {type === "oob-disk" && <OobResolution bucket="DiskOnly" />}
      {type === "oob-conflict" && <OobResolution bucket="Conflict" />}
      {type === "artist-needs-plural" && <ArtistNeedsPlural />}
      {type && !(type in RESOLUTION_TYPES) && (
        <div className="view-placeholder">Unknown resolution type: {type}</div>
      )}
    </div>
  );
}
