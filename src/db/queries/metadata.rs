//! OOB tag resolution queries and tag collision detection.

use anyhow::Result;
use rusqlite::params;

use super::Database;

/// A row from the album collision detection query:
/// (album, artist_context, isrc, catalog_number, year, date, mb_release_id).
pub type AlbumCollisionRow = (String, String, String, String, String, String, String);

// ============================================================================
// OOB signal classification
// ============================================================================
//
// A sync signal's resolution bucket is not a column — it is implied by which
// side of the diff carries the extras. Counting and paging both need the same
// rule, so it lives here once rather than being restated per query.

/// Decode a signal blob into mismatch entries.
///
/// A blob that fails to decode yields no entries, which classifies as
/// `DiskOnly` — the same fallback the OOB modal has always shown for an
/// unreadable signal.
fn decode_mismatches(blob: &[u8]) -> Vec<crate::meta::signals::data::TagMismatchEntry> {
    bincode::deserialize(blob).unwrap_or_default()
}

/// Classify a sync signal by the direction its extras point.
///
/// `VerifyTags` routes mixed-direction diffs to `signal_oob_tag_conflict`, so
/// `Conflict` is not expected here; it is returned rather than assumed away so
/// a stray row is counted somewhere rather than vanishing from the totals.
fn classify_sync(
    mismatches: &[crate::meta::signals::data::TagMismatchEntry],
) -> crate::meta::views::ConflictBucket {
    use crate::meta::views::ConflictBucket;

    let all_db_null = mismatches.iter().all(|m| m.db_value.is_none());
    let all_disk_null = mismatches.iter().all(|m| m.disk_value.is_none());
    if all_db_null {
        ConflictBucket::DiskOnly
    } else if all_disk_null {
        ConflictBucket::DbOnly
    } else {
        ConflictBucket::Conflict
    }
}

/// Classify a sync signal straight from its stored blob.
fn classify_sync_blob(blob: &[u8]) -> crate::meta::views::ConflictBucket {
    classify_sync(&decode_mismatches(blob))
}

/// Convert signal-level mismatches to view-level mismatches.
fn to_view_mismatches(
    typed: Vec<crate::meta::signals::data::TagMismatchEntry>,
) -> Vec<crate::meta::views::TagMismatchEntry> {
    typed
        .into_iter()
        .map(|m| crate::meta::views::TagMismatchEntry {
            field: m.tag_name,
            db_value: m.db_value,
            disk_value: m.disk_value,
        })
        .collect()
}

impl Database {
    // ========================================================================
    // Tag Collision Detection Queries (for canonicalization signals)
    // ========================================================================

    /// Query album data with artist context and release identifiers for collision detection.
    /// Returns file-level data to allow filtering by ISRC/catalog_number.
    /// Only considers corpus files.
    /// Returns Vec of (album_value, artist_context, isrc, catalog_number).
    ///
    pub fn get_album_data_for_collision_detection(
        &self,
        mb_release_tag_name: &str,
    ) -> Result<Vec<AlbumCollisionRow>> {
        use mm_utils::tag_names::compound_tag_sql_in;

        let album_artist_in = compound_tag_sql_in("ALBUM", "ARTIST");
        let catalog_number_in = compound_tag_sql_in("CATALOG", "NUMBER");

        // tag_name uses COLLATE NOCASE so plain `=` and `IN` are case-insensitive
        // and can use idx_corpus_tags_name(_value).
        let sql = format!(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COALESCE(isrc.tag_value, '') as isrc,
                   COALESCE(catalog.tag_value, '') as catalog_number,
                   COALESCE(year.tag_value, '') as year,
                   COALESCE(date.tag_value, '') as date,
                   COALESCE(mb_release.tag_value, '') as mb_release_id
               FROM corpus_tags album
               INNER JOIN inode_paths p ON album.inode = p.inode AND p.zone = 'corpus'
               LEFT JOIN corpus_tags album_artist
                   ON album.inode = album_artist.inode
                   AND album_artist.tag_name IN {album_artist_in}
               LEFT JOIN corpus_tags artist
                   ON album.inode = artist.inode
                   AND artist.tag_name = 'ARTIST'
               LEFT JOIN corpus_tags isrc
                   ON album.inode = isrc.inode
                   AND isrc.tag_name = 'ISRC'
               LEFT JOIN corpus_tags catalog
                   ON album.inode = catalog.inode
                   AND catalog.tag_name IN {catalog_number_in}
               LEFT JOIN corpus_tags year
                   ON album.inode = year.inode
                   AND year.tag_name = 'YEAR'
               LEFT JOIN corpus_tags date
                   ON album.inode = date.inode
                   AND date.tag_name = 'DATE'
               LEFT JOIN corpus_tags mb_release
                   ON album.inode = mb_release.inode
                   AND mb_release.tag_name = ?1
               WHERE album.tag_name = 'ALBUM'
                   AND album.tag_value IS NOT NULL
                   AND album.tag_value != ''"#
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt.query_map(params![mb_release_tag_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get corpus inodes that have BOTH configured MB track + release tags present.
    ///
    /// A file is considered "fully MB-tagged" only when both tags are present,
    /// indicating it has been matched to a specific MusicBrainz release.
    pub fn get_mb_tagged_inodes(
        &self,
        mb_track_tag_name: &str,
        mb_release_tag_name: &str,
    ) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct_track.inode
               FROM corpus_tags ct_track
               INNER JOIN inode_paths p ON ct_track.inode = p.inode AND p.zone = 'corpus'
               INNER JOIN corpus_tags ct_release
                   ON ct_track.inode = ct_release.inode
                   AND ct_release.tag_name = ?2
                   AND ct_release.tag_value IS NOT NULL
                   AND ct_release.tag_value != ''
               WHERE ct_track.tag_name = ?1
                   AND ct_track.tag_value IS NOT NULL
                   AND ct_track.tag_value != ''"#,
        )?;
        let rows = stmt.query_map(
            rusqlite::params![mb_track_tag_name, mb_release_tag_name],
            |row| row.get(0),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ========================================================================
    // Zone-Generic Tag Queries
    // ========================================================================

    /// Query distinct tag values with file counts for any tagged zone.
    /// Returns Vec of (tag_value, file_count).
    pub fn get_distinct_tag_values_for<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
    ) -> Result<Vec<(String, usize)>> {
        let sql = format!(
            r#"SELECT t.tag_value, COUNT(DISTINCT t.inode) as file_count
               FROM {} t
               INNER JOIN inode_paths p ON t.inode = p.inode AND p.zone = ?1
               WHERE t.tag_name = ?2 AND t.tag_value IS NOT NULL AND t.tag_value != ''
               GROUP BY t.tag_value
               ORDER BY file_count DESC"#,
            Z::TAG_TABLE
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![Z::ZONE_STR, tag_name], |row| {
            let value: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((value, count as usize))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get inodes that have any of the given tag values for a specific tag name in any tagged zone.
    /// Uses normalized tag name matching (strips separators) for consistency.
    pub fn get_inodes_for_tag_values_in<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
        values: &[&str],
    ) -> Result<Vec<i64>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        let normalized_tag_name = mm_utils::tag_names::normalize_tag_name(tag_name);

        let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
        let sql = format!(
            r#"SELECT DISTINCT t.inode FROM {} t
               INNER JOIN inode_paths p ON t.inode = p.inode AND p.zone = ?1
               WHERE REPLACE(REPLACE(REPLACE(REPLACE(t.tag_name, '_', ''), '-', ''), ' ', ''), '.', '') = ?2 COLLATE NOCASE
               AND t.tag_value IN ({})"#,
            Z::TAG_TABLE,
            placeholders.join(",")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(values.len() + 2);
        let zone_str = Z::ZONE_STR;
        params.push(&zone_str);
        params.push(&normalized_tag_name);
        for v in values {
            params.push(v);
        }

        let ids = stmt
            .query_map(params.as_slice(), |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;

        Ok(ids)
    }

    /// Get inodes grouped by tag value for a single tag name across multiple values.
    ///
    /// Like `get_inodes_for_tag_values_in` but returns the variant→inodes mapping
    /// in a single query instead of N. Used by canonicalization detection where
    /// each collision group has multiple variants and we want all their inodes
    /// without per-variant round trips.
    pub fn get_inodes_for_tag_values_batch<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
        values: &[&str],
    ) -> Result<std::collections::HashMap<String, Vec<i64>>> {
        if values.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let normalized_tag_name = mm_utils::tag_names::normalize_tag_name(tag_name);

        let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
        let sql = format!(
            r#"SELECT DISTINCT t.tag_value, t.inode FROM {} t
               INNER JOIN inode_paths p ON t.inode = p.inode AND p.zone = ?1
               WHERE REPLACE(REPLACE(REPLACE(REPLACE(t.tag_name, '_', ''), '-', ''), ' ', ''), '.', '') = ?2 COLLATE NOCASE
               AND t.tag_value IN ({})"#,
            Z::TAG_TABLE,
            placeholders.join(",")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(values.len() + 2);
        let zone_str = Z::ZONE_STR;
        params.push(&zone_str);
        params.push(&normalized_tag_name);
        for v in values {
            params.push(v);
        }

        let mut map: std::collections::HashMap<String, Vec<i64>> =
            std::collections::HashMap::with_capacity(values.len());
        for value in values {
            map.insert((*value).to_string(), Vec::new());
        }

        let rows = stmt.query_map(params.as_slice(), |row| {
            let value: String = row.get(0)?;
            let inode: i64 = row.get(1)?;
            Ok((value, inode))
        })?;
        for row in rows {
            let (value, inode) = row?;
            map.entry(value).or_default().push(inode);
        }

        Ok(map)
    }

    // ========================================================================
    // Album Value Queries (for embedded disc number detection)
    // ========================================================================

    /// Get ALBUM tag values with their inodes from the corpus.
    ///
    /// Returns Vec of (inode, album_value).
    /// Used by DetectDiscExtractions to find embedded disc numbers.
    pub fn get_album_values_with_inodes(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct.inode, ct.tag_value FROM corpus_tags ct
               INNER JOIN inode_paths p ON ct.inode = p.inode AND p.zone = 'corpus'
               WHERE ct.tag_name = 'ALBUM' AND ct.tag_value IS NOT NULL AND ct.tag_value != ''"#,
        )?;

        let rows = stmt.query_map(params![], |row| Ok((row.get(0)?, row.get(1)?)))?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Get TRACKNUMBER tag values with album/artist context from the corpus.
    ///
    /// Returns Vec of (inode, tracknumber, album, album_artist).
    /// Used by DetectDiscExtractions to find letter-prefixed track numbers.
    pub fn get_tracknumber_values_with_context(
        &self,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct.inode, ct.tag_value,
                      COALESCE((SELECT ct2.tag_value FROM corpus_tags ct2
                                WHERE ct2.inode = ct.inode AND ct2.tag_name = 'ALBUM'
                                LIMIT 1), ''),
                      COALESCE((SELECT ct3.tag_value FROM corpus_tags ct3
                                WHERE ct3.inode = ct.inode AND ct3.tag_name = 'ALBUM_ARTIST'
                                LIMIT 1),
                               (SELECT ct4.tag_value FROM corpus_tags ct4
                                WHERE ct4.inode = ct.inode AND ct4.tag_name = 'ALBUMARTIST'
                                LIMIT 1), '')
               FROM corpus_tags ct
               INNER JOIN inode_paths p ON ct.inode = p.inode AND p.zone = 'corpus'
               WHERE ct.tag_name = 'TRACKNUMBER' AND ct.tag_value IS NOT NULL AND ct.tag_value != ''"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // OOB Tag Resolution Queries
    // ========================================================================

    /// Count OOB signal files per resolution bucket.
    ///
    /// Counts only — no paths, no row materialization. `signal_mtime_only_mismatch`
    /// and `signal_oob_tag_conflict` map one-to-one onto a bucket, so those are
    /// plain `COUNT(*)`. A sync signal's bucket is a property of its blob (which
    /// direction carries the extras), so those rows are streamed and classified;
    /// at ~140k rows / 25 MB that scan is well under a second.
    ///
    /// No join against `inode_paths`. OOB signals are corpus-only by
    /// construction — `VerifyTags` emits them for corpus inodes and
    /// `drop_stale_corpus_signal` clears them the same way — so the zone filter
    /// excluded nothing (measured: 435,489 rows in, 435,490 out) while
    /// dominating runtime, and it double-counted inodes hardlinked into more
    /// than one corpus path.
    pub fn get_oob_bucket_counts(&self) -> Result<[usize; 4]> {
        use crate::meta::views::ConflictBucket;

        let mut counts = [0usize; 4];

        counts[ConflictBucket::MtimeOnly.index()] = self.conn.query_row(
            "SELECT COUNT(*) FROM signal_mtime_only_mismatch",
            params![],
            |row| row.get::<_, i64>(0),
        )? as usize;

        counts[ConflictBucket::Conflict.index()] = self.conn.query_row(
            "SELECT COUNT(*) FROM signal_oob_tag_conflict",
            params![],
            |row| row.get::<_, i64>(0),
        )? as usize;

        // Sync rows carry their direction inside the blob — stream and tally.
        let mut stmt = self.conn.prepare("SELECT data FROM signal_oob_tag_sync")?;
        let rows = stmt.query_map(params![], |row| row.get::<_, Vec<u8>>(0))?;
        for row in rows {
            counts[classify_sync_blob(&row?).index()] += 1;
        }

        Ok(counts)
    }

    /// Read one ordered page of a single OOB bucket.
    ///
    /// Bounded by `limit`: the modal renders bucket counts, one scrolled
    /// window, and the cursor file's mismatches, so the full set is never
    /// needed for display. Bulk resolution acts on the whole bucket
    /// server-side rather than on a client-held list of rows.
    ///
    /// Ordering is by path within the backing table. Blobs are deserialized
    /// only for rows that land in the returned page.
    pub fn get_oob_bucket_page(
        &self,
        bucket: crate::meta::views::ConflictBucket,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<crate::meta::views::OobFile>> {
        use crate::meta::views::{ConflictBucket, OobFile};

        if limit == 0 {
            return Ok(Vec::new());
        }

        match bucket {
            // One row per signal — the bucket filter is the table itself, so
            // LIMIT/OFFSET pushes all the way down to SQLite.
            ConflictBucket::MtimeOnly => {
                let mut stmt = self.conn.prepare(
                    "SELECT inode, path FROM signal_mtime_only_mismatch \
                     ORDER BY path LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
                    Ok(OobFile {
                        inode: row.get(0)?,
                        path: row.get(1)?,
                        bucket: ConflictBucket::MtimeOnly,
                        mismatches: Vec::new(),
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
            }
            ConflictBucket::Conflict => {
                let mut stmt = self.conn.prepare(
                    "SELECT inode, path, data FROM signal_oob_tag_conflict \
                     ORDER BY path LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })?;
                let mut files = Vec::new();
                for row in rows {
                    let (inode, path, blob) = row?;
                    files.push(OobFile {
                        inode,
                        path,
                        bucket: ConflictBucket::Conflict,
                        mismatches: to_view_mismatches(decode_mismatches(&blob)),
                    });
                }
                Ok(files)
            }
            // Direction lives in the blob, so SQLite cannot do the filtering:
            // walk the sync table in path order and keep the matching window.
            ConflictBucket::DbOnly | ConflictBucket::DiskOnly => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT inode, path, data FROM signal_oob_tag_sync ORDER BY path")?;
                let rows = stmt.query_map(params![], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })?;

                let mut seen = 0usize;
                let mut files = Vec::with_capacity(limit.min(1024));
                for row in rows {
                    let (inode, path, blob) = row?;
                    let typed = decode_mismatches(&blob);
                    if classify_sync(&typed) != bucket {
                        continue;
                    }
                    seen += 1;
                    if seen <= offset {
                        continue;
                    }
                    files.push(OobFile {
                        inode,
                        path,
                        bucket,
                        mismatches: to_view_mismatches(typed),
                    });
                    if files.len() >= limit {
                        break;
                    }
                }
                Ok(files)
            }
        }
    }

    /// Walk a bucket's `(inode, relative_path)` entries for bulk resolution.
    ///
    /// Keyset pagination, not `OFFSET`: bulk resolution clears signals as it
    /// goes, and those clears land through the write thread asynchronously, so
    /// row positions shift under an offset-based cursor mid-walk. Ordering by
    /// path and resuming strictly after the last path seen is stable whether or
    /// not the rows behind the cursor have been deleted yet.
    ///
    /// Carries no mismatch data — bulk resolution re-reads each file rather
    /// than trusting a diff captured when the signal was emitted.
    pub fn get_oob_bucket_entries(
        &self,
        bucket: crate::meta::views::ConflictBucket,
        after_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(i64, String)>> {
        use crate::meta::views::ConflictBucket;

        if limit == 0 {
            return Ok(Vec::new());
        }
        let after = after_path.unwrap_or("");

        let table = match bucket {
            ConflictBucket::MtimeOnly => "signal_mtime_only_mismatch",
            ConflictBucket::Conflict => "signal_oob_tag_conflict",
            ConflictBucket::DbOnly | ConflictBucket::DiskOnly => "signal_oob_tag_sync",
        };

        match bucket {
            ConflictBucket::MtimeOnly | ConflictBucket::Conflict => {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT inode, path FROM {table} WHERE path > ?1 ORDER BY path LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![after, limit as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
            }
            // Direction lives in the blob, so the filter cannot go into SQL —
            // scan forward from the cursor and keep the matching rows.
            ConflictBucket::DbOnly | ConflictBucket::DiskOnly => {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT inode, path, data FROM {table} WHERE path > ?1 ORDER BY path"
                ))?;
                let rows = stmt.query_map(params![after], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })?;
                let mut out = Vec::with_capacity(limit.min(1024));
                for row in rows {
                    let (inode, path, blob) = row?;
                    if classify_sync_blob(&blob) != bucket {
                        continue;
                    }
                    out.push((inode, path));
                    if out.len() >= limit {
                        break;
                    }
                }
                Ok(out)
            }
        }
    }

    /// Summarize files with MovedFile signals (same inode, different path).
    ///
    /// Returns a total count plus a bounded sample. The full list is never
    /// loaded for display: acknowledgement is all-or-nothing and applies as a
    /// single bulk mutation, so nothing downstream needs the per-file rows.
    pub fn get_moved_file_summary(&self) -> Result<crate::meta::views::MovedFileSummary> {
        use crate::meta::views::{MovedFileInfo, MovedFileSummary};

        let total: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM signal_moved_file", params![], |r| {
                    r.get(0)
                })?;

        let mut stmt = self.conn.prepare(
            "SELECT inode, old_path, path, old_zone, new_zone \
             FROM signal_moved_file ORDER BY path LIMIT ?1",
        )?;

        let sample: Vec<MovedFileInfo> = stmt
            .query_map(params![MovedFileSummary::SAMPLE_LIMIT as i64], |row| {
                Ok(MovedFileInfo {
                    inode: row.get(0)?,
                    old_path: row.get(1)?,
                    new_path: row.get(2)?,
                    old_zone: row.get(3)?,
                    new_zone: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(MovedFileSummary {
            total: total as usize,
            sample,
        })
    }

}
