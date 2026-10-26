//! Deploy-related health queries.

use anyhow::Result;
use rusqlite::params;

use super::super::Database;
use crate::db::types::Zone;

impl Database {
    /// Get all deploy-ready files (healthy corpus files not yet in library).
    ///
    /// Returns files with their corpus path and computed deploy path.
    /// Sorted by corpus_path for consistent display.
    pub fn get_deploy_ready_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        use crate::meta::views::DeploySignalFile;

        let mut stmt = self
            .conn
            .prepare("SELECT path, deploy_path, library_name FROM signal_deploy_ready ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
                Ok(DeploySignalFile {
                    corpus_path: row.get(0)?,
                    deploy_path: row.get(1)?,
                    library_name: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Count of healthy deployments grouped by library.
    ///
    /// `library_path` is stored as `"{library_name}/relative/path"`; the
    /// library name is derived by splitting on the first `/`. Returns
    /// `(library_name, count)` sorted by name.
    ///
    /// This replaces the older per-file loader for consumers that only need
    /// counts — the full list can run to tens of thousands of rows on a real
    /// library and nothing on the Healthy tab is actionable.
    pub fn get_deployed_healthy_counts(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT SUBSTR(library_path, 1, INSTR(library_path, '/') - 1) AS lib, COUNT(*) \
             FROM signal_deployed_healthy \
             GROUP BY lib \
             ORDER BY lib",
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let lib: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((lib, count as usize))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Healthy deployments whose `library_path` collides with a pending
    /// deploy-ready destination.
    ///
    /// Used for superseded-deployment detection: a healthy file whose library
    /// path matches a pending new deployment's destination has lost its
    /// conflict-tiebreak and needs to be moved to `leftover` so the winner
    /// can take that path. Returns entries shaped for direct extension into
    /// the leftover vec.
    ///
    /// This is a single join rather than chunked `IN (...)` batches over the
    /// deploy-ready destination list: with ~500k pending deployments that
    /// loop issued ~570 unindexed scans of `signal_deployed_healthy` and
    /// dominated deploy-view load time (~30s). The join does it in one pass.
    pub fn get_superseded_healthy(&self) -> Result<Vec<crate::meta::views::LeftoverSignalFile>> {
        use crate::meta::views::LeftoverSignalFile;

        let mut stmt = self.conn.prepare(
            "SELECT h.library_path \
             FROM signal_deployed_healthy h \
             JOIN signal_deploy_ready r \
               ON h.library_path = r.library_name || '/' || r.deploy_path \
             WHERE r.library_name != ''",
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let library_path: String = row.get(0)?;
                let library_name = library_path.split('/').next().unwrap_or("").to_string();
                Ok(LeftoverSignalFile {
                    library_name,
                    library_path,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Which of the given library paths are destinations of a pending
    /// deploy-ready file.
    ///
    /// Callers pass the (small) leftover path list to find how many leftovers
    /// are about to be replaced by an incoming deployment. Chunked over
    /// SQLite's variable limit; the input is normally in the hundreds.
    pub fn filter_paths_targeted_by_deploy_ready(
        &self,
        library_paths: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        use std::collections::HashSet;

        if library_paths.is_empty() {
            return Ok(HashSet::new());
        }

        const CHUNK: usize = 900;
        let mut found = HashSet::new();

        for chunk in library_paths.chunks(CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT library_name || '/' || deploy_path AS dest \
                 FROM signal_deploy_ready \
                 WHERE library_name != '' AND dest IN ({})",
                placeholders,
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params_iter = rusqlite::params_from_iter(chunk.iter().map(|s| s.as_str()));
            let rows = stmt
                .query_map(params_iter, |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            found.extend(rows);
        }

        Ok(found)
    }

    /// Deploy-ready counts grouped by library, sorted by name.
    pub fn get_deploy_ready_counts_by_library(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT library_name, COUNT(*) \
             FROM signal_deploy_ready \
             GROUP BY library_name \
             ORDER BY library_name",
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let lib: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((lib, count as usize))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Deploy-ready counts grouped by corpus parent directory.
    ///
    /// The directory expression is the SQLite equivalent of
    /// `Path::new(path).parent()`: `rtrim` strips trailing characters that
    /// appear in the path's non-slash character set, which eats the final
    /// segment and stops at the last `/`; the outer `substr` drops that
    /// trailing separator. Paths with no `/` yield `""`, matching
    /// `Path::parent()` on a bare filename.
    ///
    /// Aggregating in SQL keeps the deploy view off the ~500k-row full
    /// materialization it used to do just to bucket rows by directory.
    pub fn get_deploy_ready_counts_by_directory(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT CASE WHEN INSTR(path, '/') = 0 THEN '' \
                    ELSE SUBSTR(rtrim(path, REPLACE(path, '/', '')), 1, \
                                LENGTH(rtrim(path, REPLACE(path, '/', ''))) - 1) \
                    END AS dir, \
                    COUNT(*) \
             FROM signal_deploy_ready \
             GROUP BY dir",
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let dir: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((dir, count as usize))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all sidecar images ready for deployment.
    ///
    /// Reads precomputed signals emitted by DeriveCorpusDeployStatus.
    pub fn get_sidecar_deploy_ready_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::SidecarDeployReadySignal>> {
        use crate::meta::signals::data::{SidecarDeployReadyData, SidecarDeployReadySignal};

        let mut stmt = self.conn.prepare(
            "SELECT inode, path, deploy_path, library_name, data FROM signal_sidecar_deploy_ready ORDER BY path"
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let blob: Vec<u8> = row.get(4)?;
                let data: SidecarDeployReadyData = bincode::deserialize(&blob).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Blob,
                        Box::new(e),
                    )
                })?;
                Ok(SidecarDeployReadySignal {
                    inode: row.get(0)?,
                    path: row.get(1)?,
                    deploy_path: row.get(2)?,
                    library_name: row.get(3)?,
                    data,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all stale library files (deployed at wrong path due to tag changes).
    ///
    /// Returns files with their current library path and expected path.
    /// Sorted by library_path for consistent display.
    pub fn get_library_stale_files(&self) -> Result<Vec<crate::meta::views::StaleSignalFile>> {
        use crate::meta::views::StaleSignalFile;

        let mut stmt = self.conn.prepare(
            "SELECT library_path, expected_path FROM signal_library_stale ORDER BY library_path",
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let library_path: String = row.get(0)?;
                // library_path is "{library_name}/relative/path" — extract library_name
                let library_name = library_path.split('/').next().unwrap_or("").to_string();
                Ok(StaleSignalFile {
                    library_name,
                    library_path,
                    expected_path: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all leftover library files (no corpus backing).
    ///
    /// Sorted by library_path for consistent display.
    pub fn get_library_leftover_files(
        &self,
    ) -> Result<Vec<crate::meta::views::LeftoverSignalFile>> {
        use crate::meta::views::LeftoverSignalFile;

        // key = "library_leftover:{library_name}:{library_name}/path/..."
        let mut stmt = self
            .conn
            .prepare("SELECT key FROM signal_library_leftover ORDER BY key")?;

        let results = stmt
            .query_map(params![], |row| {
                let key: String = row.get(0)?;
                let (library_name, library_path) =
                    crate::meta::signals::data::LibraryLeftoverSignal::parse_key(&key)
                        .map(|(n, p)| (n.to_string(), p.to_string()))
                        .unwrap_or_else(|| (String::new(), key.clone()));
                Ok(LeftoverSignalFile {
                    library_name,
                    library_path,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Inodes that lost a deploy-path tiebreak.
    ///
    /// For each `signal_deploy_conflict` row, the BLOB encodes the inodes
    /// involved in the conflict with `inodes[0]` as the alphabetical tiebreak
    /// winner. This helper returns the set of non-winner inodes — i.e. corpus
    /// inodes whose deployments are superseded and whose library hardlinks
    /// must be stashed before the winner is deployed.
    pub fn get_deploy_conflict_loser_inodes(&self) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM signal_deploy_conflict")?;
        let rows = stmt.query_map(params![], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;
        let mut losers = std::collections::HashSet::new();
        for blob in rows {
            let blob = blob?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();
            // inodes[0] is the winner; the rest lost the tiebreak.
            for &inode in inodes.iter().skip(1) {
                losers.insert(inode);
            }
        }
        Ok(losers)
    }

    /// Get all deploy conflict groups (multiple corpus files → same library path).
    ///
    /// Sorted by key for consistent display.
    pub fn get_deploy_conflict_groups(&self) -> Result<Vec<crate::meta::views::ConflictGroup>> {
        use crate::meta::views::ConflictGroup;

        let mut stmt = self
            .conn
            .prepare("SELECT key, data FROM signal_deploy_conflict ORDER BY key")?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((deploy_path, blob))
        })?;

        for row in rows {
            let (deploy_path, blob) = row?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();

            let mut conflicting_files = Vec::new();
            for inode in inodes {
                if let Ok(Some(audio_file)) = self.get_audio_file_by_inode(inode, Zone::Corpus) {
                    conflicting_files.push((audio_file.path().to_string(), inode));
                }
            }

            results.push(ConflictGroup {
                deploy_path,
                conflicting_files,
            });
        }

        Ok(results)
    }

    /// Get all sidecar deploy conflict groups (multiple corpus images → same library path).
    ///
    /// Sorted by library_name/deploy_path for consistent display.
    pub fn get_sidecar_conflict_groups(
        &self,
    ) -> Result<Vec<crate::meta::views::SidecarConflictGroup>> {
        use crate::meta::views::SidecarConflictGroup;

        let mut stmt = self.conn.prepare(
            "SELECT deploy_path, library_name, data FROM signal_sidecar_deploy_conflict ORDER BY library_name, deploy_path"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let library_name: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((deploy_path, library_name, blob))
        })?;

        for row in rows {
            let (deploy_path, library_name, blob) = row?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();

            let mut conflicting_files = Vec::new();
            for inode in inodes {
                if let Ok(Some(path)) = self.get_path_for_inode::<crate::zones::CorpusZone>(inode) {
                    conflicting_files.push((path, inode));
                }
            }

            results.push(SidecarConflictGroup {
                deploy_path,
                library_name,
                conflicting_files,
            });
        }

        Ok(results)
    }

    /// Get deploy status for the Deploy view and titlebar indicator.
    ///
    /// Returns whether there's actionable deploy work and per-library file counts.
    /// `configured_libraries` limits counting to only the configured deployment
    /// directories, ignoring other top-level dirs under the library root.
    pub fn get_deploy_status(
        &self,
        configured_libraries: &[String],
    ) -> Result<crate::meta::views::DeployStatus> {
        let needs_action: bool = self.conn.query_row(
            "SELECT
                EXISTS(SELECT 1 FROM signal_deploy_ready)
                OR EXISTS(SELECT 1 FROM signal_library_stale)
                OR EXISTS(SELECT 1 FROM signal_library_leftover)
                OR EXISTS(SELECT 1 FROM signal_sidecar_deploy_ready)",
            [],
            |row| row.get(0),
        )?;

        // Per-library file counts: only count files in configured library dirs
        let mut library_file_counts = Vec::new();
        for lib_name in configured_libraries {
            let pattern = super::super::dir_like_pattern_str(lib_name);
            let count: usize = self.conn.query_row(
                "SELECT COUNT(*) FROM inode_paths WHERE zone = 'library' AND path LIKE ?1 ESCAPE '\\'",
                params![pattern],
                |row| row.get(0),
            )?;
            library_file_counts.push((lib_name.clone(), count));
        }
        library_file_counts.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(crate::meta::views::DeployStatus {
            needs_action,
            library_file_counts,
        })
    }
}
