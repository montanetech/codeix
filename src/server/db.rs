use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::index::format::{FileEntry, ReferenceEntry, SymbolEntry, TextEntry};

/// Convert visibility string to integer level for filtering.
///
/// Visibility hierarchy (lower = more visible):
/// - 1 = public
/// - 2 = internal
/// - 3 = private (or NULL/unknown, or files with no symbols)
///
/// Filter means "at most this level": visibility_level <= N
/// - "public" → level <= 1 (only public)
/// - "internal" → level <= 2 (public + internal)
/// - "private" → no filter (all symbols)
///
/// Files with no symbols are treated as private (level 3) - they have no public API.
///
/// When `visibility` is None, the `default` is used.
pub fn visibility_max_level(visibility: Option<&str>, default: &str) -> Option<i32> {
    let effective = visibility.unwrap_or(default);
    match effective {
        "public" => Some(1),
        "internal" => Some(2),
        "private" => None, // No filter (all)
        _ => None,         // Unknown, no filter
    }
}

/// Convert visibility string to integer level for storage.
fn visibility_to_level(visibility: Option<&str>) -> i32 {
    match visibility {
        Some("public") => 1,
        Some("internal") => 2,
        _ => 3, // private or unknown
    }
}

/// Unified search result with type discriminator.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SearchResult {
    Symbol(SymbolEntry),
    File(FileEntry),
    Text(TextEntry),
}

/// An in-memory SQLite database with FTS5 virtual tables for fast text search
/// over the code index.
pub struct SearchDb {
    conn: Connection,
    /// Whether FTS5 virtual tables are enabled. Disabled in build mode to save memory.
    fts_enabled: bool,
}

impl SearchDb {
    /// Create a new in-memory database with FTS5 enabled (for serve mode).
    pub fn new() -> Result<Self> {
        Self::new_internal(true)
    }

    /// Create a new in-memory database without FTS5 (for build mode).
    /// This significantly reduces memory usage for large repositories.
    pub fn new_no_fts() -> Result<Self> {
        Self::new_internal(false)
    }

    /// Internal constructor with configurable FTS support.
    fn new_internal(fts_enabled: bool) -> Result<Self> {
        let conn = Connection::open_in_memory()?;

        // Content tables (store the actual data for retrieval)
        conn.execute_batch(
            "
            CREATE TABLE files (
                project     TEXT NOT NULL,
                path        TEXT NOT NULL,
                parent_path TEXT NOT NULL,
                lang        TEXT,
                hash        TEXT NOT NULL,
                lines       INTEGER NOT NULL,
                title       TEXT,
                description TEXT,
                PRIMARY KEY (project, path)
            );
            CREATE INDEX idx_files_parent ON files (project, parent_path);

            CREATE TABLE symbols (
                project       TEXT NOT NULL,
                file       TEXT NOT NULL,
                name       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end   INTEGER NOT NULL,
                parent     TEXT,
                tokens     TEXT,
                alias      TEXT,
                visibility TEXT,
                visibility_level INTEGER NOT NULL DEFAULT 3
            );

            CREATE TABLE texts (
                project       TEXT NOT NULL,
                file       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end   INTEGER NOT NULL,
                text       TEXT NOT NULL,
                parent     TEXT
            );

            CREATE TABLE refs (
                project       TEXT NOT NULL,
                file          TEXT NOT NULL,
                name          TEXT NOT NULL,
                kind          TEXT NOT NULL,
                line_start    INTEGER NOT NULL,
                line_end      INTEGER NOT NULL,
                caller        TEXT
            );

            -- Indexes for exact lookups
            CREATE INDEX idx_symbols_project_file ON symbols(project, file);
            CREATE INDEX idx_symbols_project_file_parent ON symbols(project, file, parent);
            CREATE INDEX idx_symbols_project_file_kind ON symbols(project, file, kind);
            CREATE INDEX idx_symbols_visibility ON symbols(project, visibility_level);
            CREATE INDEX idx_texts_project_file ON texts(project, file);
            CREATE INDEX idx_files_project ON files(project);
            CREATE INDEX idx_symbols_project ON symbols(project);
            CREATE INDEX idx_texts_project ON texts(project);

            -- Indexes for reference queries
            CREATE INDEX idx_refs_project_name ON refs(project, name);
            CREATE INDEX idx_refs_project_caller ON refs(project, caller);
            CREATE INDEX idx_refs_project_file ON refs(project, file);
            CREATE INDEX idx_refs_project_name_kind ON refs(project, name, kind);
            ",
        )
        .context("failed to create database schema")?;

        // Unified FTS5 virtual table for full-text search (only when enabled)
        // Three searchable columns with BM25 weighting: name (3x), file (2x), content (1x)
        if fts_enabled {
            conn.execute_batch(
                "
                CREATE VIRTUAL TABLE search_fts USING fts5(
                    name,               -- symbol/file name (highest weight)
                    file,               -- file path (medium weight)
                    content,            -- tokens, docstrings, etc. (lower weight)
                    type UNINDEXED,     -- 'symbol', 'file', 'text'
                    rowid_ref UNINDEXED,-- rowid in source table
                    path UNINDEXED,     -- file path (for GLOB filtering)
                    kind UNINDEXED,     -- symbol/text kind, or file lang
                    project UNINDEXED,  -- project filter
                    visibility_level UNINDEXED -- 1=public, 2=internal, 3=private (0 for files/texts)
                );
                ",
            )
            .context("failed to create FTS5 table")?;
        }

        Ok(Self { conn, fts_enabled })
    }

    /// Load index data into the database for a specific project.
    pub fn load(
        &self,
        project: &str,
        files: &[FileEntry],
        symbols: &[SymbolEntry],
        texts: &[TextEntry],
        references: &[ReferenceEntry],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Insert files
        {
            let mut stmt = tx.prepare(
                "INSERT INTO files (project, path, parent_path, lang, hash, lines, title, description) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for f in files {
                let parent_path = match f.path.rfind('/') {
                    Some(pos) => &f.path[..pos],
                    None => ".",
                };
                stmt.execute(rusqlite::params![
                    project,
                    f.path,
                    parent_path,
                    f.lang,
                    f.hash,
                    f.lines,
                    f.title,
                    f.description
                ])?;
            }
        }

        // Insert symbols
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols (project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility, visibility_level)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for s in symbols {
                stmt.execute(rusqlite::params![
                    project,
                    s.file,
                    s.name,
                    s.kind,
                    s.line[0],
                    s.line[1],
                    s.parent,
                    s.tokens,
                    s.alias,
                    s.visibility,
                    visibility_to_level(s.visibility.as_deref()),
                ])?;
            }
        }

        // Insert texts
        {
            let mut stmt = tx.prepare(
                "INSERT INTO texts (project, file, kind, line_start, line_end, text, parent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for t in texts {
                stmt.execute(rusqlite::params![
                    project, t.file, t.kind, t.line[0], t.line[1], t.text, t.parent,
                ])?;
            }
        }

        // Insert references
        {
            let mut stmt = tx.prepare(
                "INSERT INTO refs (project, file, name, kind, line_start, line_end, caller)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for r in references {
                stmt.execute(rusqlite::params![
                    project, r.file, r.name, r.kind, r.line[0], r.line[1], r.caller,
                ])?;
            }
        }

        // Populate unified FTS5 index from content tables (only when FTS enabled)
        // BM25 weights: name (3x), file (2x), content (1x)
        if self.fts_enabled {
            tx.execute_batch(
                "
                -- Insert files: name=title, file=path, content=description
                INSERT INTO search_fts(name, file, content, type, rowid_ref, path, kind, project, visibility_level)
                SELECT
                    COALESCE(title, ''),
                    COALESCE(path, ''),
                    COALESCE(description, ''),
                    'file',
                    rowid,
                    path,
                    lang,
                    project,
                    0
                FROM files;

                -- Insert symbols: name=symbol name, file=path, content=kind + tokens
                INSERT INTO search_fts(name, file, content, type, rowid_ref, path, kind, project, visibility_level)
                SELECT
                    COALESCE(name, ''),
                    COALESCE(file, ''),
                    COALESCE(kind, '') || ' ' || COALESCE(tokens, ''),
                    'symbol',
                    rowid,
                    file,
                    kind,
                    project,
                    visibility_level
                FROM symbols;

                -- Insert texts: name=empty, file=path, content=text
                INSERT INTO search_fts(name, file, content, type, rowid_ref, path, kind, project, visibility_level)
                SELECT
                    '',
                    COALESCE(file, ''),
                    COALESCE(text, ''),
                    'text',
                    rowid,
                    file,
                    kind,
                    project,
                    0
                FROM texts;
                ",
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Unified search across symbols, files, and texts.
    ///
    /// Parameters:
    /// - query: FTS5 search query (supports * wildcards)
    /// - scope: Types to search ("symbol", "file", "text"). Empty = all.
    /// - kind: Filter by kind (symbol kind, text kind, or file lang)
    /// - path: Filter by file path (supports GLOB patterns with *)
    /// - project: Filter by project
    /// - visibility: Minimum visibility level for symbols ("public", "internal", or "private"/None)
    /// - limit: Max results (default 100)
    /// - offset: Pagination offset
    ///
    /// Returns results ordered by BM25 relevance.
    ///
    /// The visibility filter only applies to symbol results (files and texts pass through).
    /// Filtering is done directly in the FTS5 query using the visibility column.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        query: &str,
        scope: &[String],
        kind: &[String],
        path: Option<&str>,
        project: Option<&str>,
        visibility: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SearchResult>> {
        // Build FTS5 MATCH expression (searches all columns: name, file, content)
        let fts_query = fts5_quote(query);

        // Build WHERE clause for filters
        let mut conditions = vec!["search_fts MATCH ?1".to_string()];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts_query)];

        // Scope filter (type)
        if !scope.is_empty() {
            let placeholders: Vec<String> = scope
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect();
            conditions.push(format!("type IN ({})", placeholders.join(", ")));
            for s in scope {
                params.push(Box::new(s.clone()));
            }
        }

        // Kind filter
        if !kind.is_empty() {
            let start_param = params.len() + 1;
            let placeholders: Vec<String> = kind
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", start_param + i))
                .collect();
            conditions.push(format!("kind IN ({})", placeholders.join(", ")));
            for k in kind {
                params.push(Box::new(k.clone()));
            }
        }

        let next_param = params.len() + 1;

        // Path filter (supports GLOB)
        if let Some(p) = path {
            if p.contains('*') {
                conditions.push(format!("path GLOB ?{}", next_param));
            } else {
                conditions.push(format!("path = ?{}", next_param));
            }
            params.push(Box::new(p.to_string()));
        }

        let next_param = params.len() + 1;

        // Project filter
        if let Some(proj) = project {
            conditions.push(format!("project = ?{}", next_param));
            params.push(Box::new(proj.to_string()));
        }

        let next_param = params.len() + 1;

        // Visibility filter: visibility_level <= max_level
        // Files/texts have level 0 (always pass), symbols have 1/2/3
        if let Some(max_level) = visibility_max_level(visibility, "public") {
            conditions.push(format!("visibility_level <= ?{}", next_param));
            params.push(Box::new(max_level));
        }

        // Add exact match parameter for boosting
        let exact_param = params.len() + 1;
        // Extract first word from query for exact match comparison (lowercase)
        let exact_term = query
            .split_whitespace()
            .next()
            .unwrap_or(query)
            .to_lowercase();
        params.push(Box::new(exact_term));

        // Add limit and offset
        let limit_param = params.len() + 1;
        let offset_param = params.len() + 2;
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        // BM25 weights: name (3x), file (2x), content (1x)
        // Boost exact name matches with CASE (bm25 returns negative, so -1000 ranks first)
        // Secondary sort by name length to prefer shorter matches
        let sql = format!(
            "SELECT type, rowid_ref FROM search_fts WHERE {} \
             ORDER BY CASE WHEN lower(name) = ?{} THEN -1000 ELSE 0 END + bm25(search_fts, 3.0, 2.0, 1.0), length(name) \
             LIMIT ?{} OFFSET ?{}",
            conditions.join(" AND "),
            exact_param,
            limit_param,
            offset_param
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs), |row| {
            let entry_type: String = row.get(0)?;
            let rowid: i64 = row.get(1)?;
            Ok((entry_type, rowid))
        })?;

        // Collect (type, rowid) pairs
        let mut type_rowid_pairs = Vec::new();
        for row in rows {
            type_rowid_pairs.push(row?);
        }

        // Fetch full records from content tables
        let mut results = Vec::new();
        for (entry_type, rowid) in type_rowid_pairs {
            let result = match entry_type.as_str() {
                "symbol" => self.get_symbol_by_rowid(rowid).map(SearchResult::Symbol)?,
                "file" => self.get_file_by_rowid(rowid).map(SearchResult::File)?,
                "text" => self.get_text_by_rowid(rowid).map(SearchResult::Text)?,
                _ => continue,
            };
            results.push(result);
        }

        Ok(results)
    }

    /// Fetch a symbol by rowid.
    fn get_symbol_by_rowid(&self, rowid: i64) -> Result<SymbolEntry> {
        let mut stmt = self.conn.prepare(
            "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
             FROM symbols WHERE rowid = ?1",
        )?;
        stmt.query_row([rowid], |row| {
            Ok(SymbolEntry {
                project: row.get(0)?,
                file: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                line: [row.get(4)?, row.get(5)?],
                parent: row.get(6)?,
                tokens: row.get(7)?,
                alias: row.get(8)?,
                visibility: row.get(9)?,
            })
        })
        .context("failed to fetch symbol by rowid")
    }

    /// Fetch a file by rowid.
    fn get_file_by_rowid(&self, rowid: i64) -> Result<FileEntry> {
        let mut stmt = self.conn.prepare(
            "SELECT project, path, lang, hash, lines, title, description
             FROM files WHERE rowid = ?1",
        )?;
        stmt.query_row([rowid], |row| {
            Ok(FileEntry {
                project: row.get(0)?,
                path: row.get(1)?,
                lang: row.get(2)?,
                hash: row.get(3)?,
                lines: row.get(4)?,
                title: row.get(5)?,
                description: row.get(6)?,
            })
        })
        .context("failed to fetch file by rowid")
    }

    /// Fetch a text by rowid.
    fn get_text_by_rowid(&self, rowid: i64) -> Result<TextEntry> {
        let mut stmt = self.conn.prepare(
            "SELECT project, file, kind, line_start, line_end, text, parent
             FROM texts WHERE rowid = ?1",
        )?;
        stmt.query_row([rowid], |row| {
            Ok(TextEntry {
                project: row.get(0)?,
                file: row.get(1)?,
                kind: row.get(2)?,
                line: [row.get(3)?, row.get(4)?],
                text: row.get(5)?,
                parent: row.get(6)?,
            })
        })
        .context("failed to fetch text by rowid")
    }

    /// Get all symbols in a file, ordered by start line.
    ///
    /// If visibility is specified, only symbols at that visibility level or higher are returned.
    /// Hierarchy: public > internal > private.
    pub fn get_file_symbols(
        &self,
        file: &str,
        visibility: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SymbolEntry>> {
        let max_level = visibility_max_level(visibility, "public");

        let sql = match max_level {
            Some(_) => {
                "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
                 FROM symbols
                 WHERE file = ?1 AND visibility_level <= ?2
                 ORDER BY line_start
                 LIMIT ?3 OFFSET ?4"
            }
            None => {
                "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
                 FROM symbols
                 WHERE file = ?1
                 ORDER BY line_start
                 LIMIT ?2 OFFSET ?3"
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let rows: Vec<SymbolEntry> = match max_level {
            Some(level) => stmt
                .query_map(rusqlite::params![file, level, limit, offset], |row| {
                    Ok(SymbolEntry {
                        project: row.get(0)?,
                        file: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        line: [row.get(4)?, row.get(5)?],
                        parent: row.get(6)?,
                        tokens: row.get(7)?,
                        alias: row.get(8)?,
                        visibility: row.get(9)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(rusqlite::params![file, limit, offset], |row| {
                    Ok(SymbolEntry {
                        project: row.get(0)?,
                        file: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        line: [row.get(4)?, row.get(5)?],
                        parent: row.get(6)?,
                        tokens: row.get(7)?,
                        alias: row.get(8)?,
                        visibility: row.get(9)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };

        Ok(rows)
    }

    /// Get direct children of a symbol in a file.
    ///
    /// If visibility is specified, only symbols at that visibility level or higher are returned.
    /// Hierarchy: public > internal > private.
    pub fn get_children(
        &self,
        file: &str,
        parent: &str,
        visibility: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SymbolEntry>> {
        let max_level = visibility_max_level(visibility, "public");

        let sql = match max_level {
            Some(_) => {
                "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
                 FROM symbols
                 WHERE file = ?1 AND parent = ?2 AND visibility_level <= ?3
                 ORDER BY line_start
                 LIMIT ?4 OFFSET ?5"
            }
            None => {
                "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
                 FROM symbols
                 WHERE file = ?1 AND parent = ?2
                 ORDER BY line_start
                 LIMIT ?3 OFFSET ?4"
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let rows: Vec<SymbolEntry> = match max_level {
            Some(level) => stmt
                .query_map(
                    rusqlite::params![file, parent, level, limit, offset],
                    |row| {
                        Ok(SymbolEntry {
                            project: row.get(0)?,
                            file: row.get(1)?,
                            name: row.get(2)?,
                            kind: row.get(3)?,
                            line: [row.get(4)?, row.get(5)?],
                            parent: row.get(6)?,
                            tokens: row.get(7)?,
                            alias: row.get(8)?,
                            visibility: row.get(9)?,
                        })
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(rusqlite::params![file, parent, limit, offset], |row| {
                    Ok(SymbolEntry {
                        project: row.get(0)?,
                        file: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        line: [row.get(4)?, row.get(5)?],
                        parent: row.get(6)?,
                        tokens: row.get(7)?,
                        alias: row.get(8)?,
                        visibility: row.get(9)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };

        Ok(rows)
    }

    /// Get all references TO a symbol (who calls/uses this symbol).
    /// Returns references sorted by file, then line.
    ///
    /// Supports flexible name matching:
    /// - Exact match: "self.handle_exception" matches "self.handle_exception"
    /// - Base name match: "handle_exception" matches "self.handle_exception"
    ///
    /// This handles OOP patterns where references are stored with receiver prefixes
    /// (self., this., etc.) but users query with base method names.
    ///
    /// If visibility is specified, only references to symbols at that visibility level
    /// or higher are returned. The target symbol's visibility is looked up in the symbols table.
    pub fn get_callers(
        &self,
        name: &str,
        kind: Option<&str>,
        project: Option<&str>,
        visibility: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ReferenceEntry>> {
        let max_level = visibility_max_level(visibility, "private");

        // Match exact name OR names ending with .{name} (e.g., "self.foo" matches query "foo")
        let mut conditions = vec!["(r.name = ? OR r.name LIKE ?)".to_string()];
        let like_pattern = format!("%.{}", name);
        let mut params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(name.to_string()), Box::new(like_pattern)];

        if let Some(k) = kind {
            conditions.push("r.kind = ?".to_string());
            params.push(Box::new(k.to_string()));
        }
        if let Some(p) = project {
            conditions.push("r.project = ?".to_string());
            params.push(Box::new(p.to_string()));
        }

        // Visibility filter: join with symbols to filter by target symbol's visibility_level
        let sql = if let Some(level) = max_level {
            params.push(Box::new(level));
            params.push(Box::new(limit));
            params.push(Box::new(offset));

            format!(
                "SELECT DISTINCT r.project, r.file, r.name, r.kind, r.line_start, r.line_end, r.caller
                 FROM refs r
                 INNER JOIN symbols s ON (s.name = r.name OR s.name LIKE '%.' || r.name)
                                      AND s.project = r.project
                 WHERE {} AND s.visibility_level <= ?
                 ORDER BY r.file, r.line_start
                 LIMIT ? OFFSET ?",
                conditions.join(" AND ")
            )
        } else {
            // No visibility filter, simple query
            params.push(Box::new(limit));
            params.push(Box::new(offset));

            format!(
                "SELECT r.project, r.file, r.name, r.kind, r.line_start, r.line_end, r.caller
                 FROM refs r
                 WHERE {}
                 ORDER BY r.file, r.line_start
                 LIMIT ? OFFSET ?",
                conditions.join(" AND ")
            )
        };

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs), |row| {
            Ok(ReferenceEntry {
                project: row.get(0)?,
                file: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                line: [row.get(4)?, row.get(5)?],
                caller: row.get(6)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all references FROM a symbol (what does this symbol call/use).
    /// Returns references sorted by file, then line.
    ///
    /// Supports flexible caller name matching:
    /// - Exact match: "MyClass.method" matches "MyClass.method"
    /// - Base name match: "method" matches "MyClass.method"
    ///
    /// This handles qualified names where the caller context includes class prefixes.
    ///
    /// If visibility is specified, only references to symbols at that visibility level
    /// or higher are returned. The referenced symbol's visibility is looked up in the symbols table.
    pub fn get_callees(
        &self,
        caller: &str,
        kind: Option<&str>,
        project: Option<&str>,
        visibility: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ReferenceEntry>> {
        let max_level = visibility_max_level(visibility, "private");

        // Match exact caller OR callers ending with .{caller} (e.g., "Class.method" matches query "method")
        let mut conditions = vec!["(r.caller = ? OR r.caller LIKE ?)".to_string()];
        let like_pattern = format!("%.{}", caller);
        let mut params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(caller.to_string()), Box::new(like_pattern)];

        if let Some(k) = kind {
            conditions.push("r.kind = ?".to_string());
            params.push(Box::new(k.to_string()));
        }
        if let Some(p) = project {
            conditions.push("r.project = ?".to_string());
            params.push(Box::new(p.to_string()));
        }

        // Visibility filter: join with symbols to filter by referenced symbol's visibility_level
        let sql = if let Some(level) = max_level {
            params.push(Box::new(level));
            params.push(Box::new(limit));
            params.push(Box::new(offset));

            format!(
                "SELECT DISTINCT r.project, r.file, r.name, r.kind, r.line_start, r.line_end, r.caller
                 FROM refs r
                 INNER JOIN symbols s ON (s.name = r.name OR s.name LIKE '%.' || r.name)
                                      AND s.project = r.project
                 WHERE {} AND s.visibility_level <= ?
                 ORDER BY r.file, r.line_start
                 LIMIT ? OFFSET ?",
                conditions.join(" AND ")
            )
        } else {
            // No visibility filter, simple query
            params.push(Box::new(limit));
            params.push(Box::new(offset));

            format!(
                "SELECT r.project, r.file, r.name, r.kind, r.line_start, r.line_end, r.caller
                 FROM refs r
                 WHERE {}
                 ORDER BY r.file, r.line_start
                 LIMIT ? OFFSET ?",
                conditions.join(" AND ")
            )
        };

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs), |row| {
            Ok(ReferenceEntry {
                project: row.get(0)?,
                file: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                line: [row.get(4)?, row.get(5)?],
                caller: row.get(6)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get the hash of a file from the DB (for change detection).
    pub fn get_file_hash(&self, project: &str, path: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT hash FROM files WHERE project = ?1 AND path = ?2")?;
        let mut rows = stmt.query(rusqlite::params![project, path])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Remove all data for a file (from files, symbols, texts, refs tables).
    /// Does not rebuild FTS indexes - caller should call rebuild_fts() after batch operations.
    pub fn remove_file(&self, project: &str, path: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "DELETE FROM files WHERE project = ?1 AND path = ?2",
            rusqlite::params![project, path],
        )?;
        tx.execute(
            "DELETE FROM symbols WHERE project = ?1 AND file = ?2",
            rusqlite::params![project, path],
        )?;
        tx.execute(
            "DELETE FROM texts WHERE project = ?1 AND file = ?2",
            rusqlite::params![project, path],
        )?;
        tx.execute(
            "DELETE FROM refs WHERE project = ?1 AND file = ?2",
            rusqlite::params![project, path],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Remove all data for a project (files, symbols, texts, refs).
    /// Does not rebuild FTS indexes - caller should call rebuild_fts() after batch operations.
    pub fn remove_project(&self, project: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "DELETE FROM files WHERE project = ?1",
            rusqlite::params![project],
        )?;
        tx.execute(
            "DELETE FROM symbols WHERE project = ?1",
            rusqlite::params![project],
        )?;
        tx.execute(
            "DELETE FROM texts WHERE project = ?1",
            rusqlite::params![project],
        )?;
        tx.execute(
            "DELETE FROM refs WHERE project = ?1",
            rusqlite::params![project],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Upsert a single file and its symbols/texts/references.
    /// Removes old data for this path first, then inserts new data.
    /// Does not rebuild FTS indexes - caller should call rebuild_fts() after batch operations.
    pub fn upsert_file(
        &self,
        project: &str,
        file: &FileEntry,
        symbols: &[SymbolEntry],
        texts: &[TextEntry],
        references: &[ReferenceEntry],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Remove old data for this file
        tx.execute(
            "DELETE FROM files WHERE project = ?1 AND path = ?2",
            rusqlite::params![project, &file.path],
        )?;
        tx.execute(
            "DELETE FROM symbols WHERE project = ?1 AND file = ?2",
            rusqlite::params![project, &file.path],
        )?;
        tx.execute(
            "DELETE FROM texts WHERE project = ?1 AND file = ?2",
            rusqlite::params![project, &file.path],
        )?;
        tx.execute(
            "DELETE FROM refs WHERE project = ?1 AND file = ?2",
            rusqlite::params![project, &file.path],
        )?;

        // Insert file
        let parent_path = match file.path.rfind('/') {
            Some(pos) => &file.path[..pos],
            None => ".",
        };
        tx.execute(
            "INSERT INTO files (project, path, parent_path, lang, hash, lines, title, description) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![project, file.path, parent_path, file.lang, file.hash, file.lines, file.title, file.description],
        )?;

        // Insert symbols
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols (project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility, visibility_level)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for s in symbols {
                stmt.execute(rusqlite::params![
                    project,
                    s.file,
                    s.name,
                    s.kind,
                    s.line[0],
                    s.line[1],
                    s.parent,
                    s.tokens,
                    s.alias,
                    s.visibility,
                    visibility_to_level(s.visibility.as_deref()),
                ])?;
            }
        }

        // Insert texts
        {
            let mut stmt = tx.prepare(
                "INSERT INTO texts (project, file, kind, line_start, line_end, text, parent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for t in texts {
                stmt.execute(rusqlite::params![
                    project, t.file, t.kind, t.line[0], t.line[1], t.text, t.parent,
                ])?;
            }
        }

        // Insert references
        {
            let mut stmt = tx.prepare(
                "INSERT INTO refs (project, file, name, kind, line_start, line_end, caller)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for r in references {
                stmt.execute(rusqlite::params![
                    project, r.file, r.name, r.kind, r.line[0], r.line[1], r.caller,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Rebuild unified FTS5 index.
    /// Call this after batch upsert/remove operations.
    /// No-op when FTS is disabled (build mode).
    pub fn rebuild_fts(&self) -> Result<()> {
        if !self.fts_enabled {
            return Ok(());
        }
        self.conn.execute_batch(
            "
            DELETE FROM search_fts;

            -- Insert files: name=title, file=path, content=description
            INSERT INTO search_fts(name, file, content, type, rowid_ref, path, kind, project, visibility_level)
            SELECT
                COALESCE(title, ''),
                COALESCE(path, ''),
                COALESCE(description, ''),
                'file',
                rowid,
                path,
                lang,
                project,
                0
            FROM files;

            -- Insert symbols: name=symbol name, file=path, content=kind + tokens
            INSERT INTO search_fts(name, file, content, type, rowid_ref, path, kind, project, visibility_level)
            SELECT
                COALESCE(name, ''),
                COALESCE(file, ''),
                COALESCE(kind, '') || ' ' || COALESCE(tokens, ''),
                'symbol',
                rowid,
                file,
                kind,
                project,
                visibility_level
            FROM symbols;

            -- Insert texts: name=empty, file=path, content=text
            INSERT INTO search_fts(name, file, content, type, rowid_ref, path, kind, project, visibility_level)
            SELECT
                '',
                COALESCE(file, ''),
                COALESCE(text, ''),
                'text',
                rowid,
                file,
                kind,
                project,
                0
            FROM texts;
            ",
        )?;
        Ok(())
    }

    /// Export all data from DB back to vecs (for flushing to disk).
    #[allow(clippy::type_complexity)]
    pub fn export_all(
        &self,
    ) -> Result<(
        Vec<FileEntry>,
        Vec<SymbolEntry>,
        Vec<TextEntry>,
        Vec<ReferenceEntry>,
    )> {
        let mut files = Vec::new();
        let mut symbols = Vec::new();
        let mut texts = Vec::new();
        let mut references = Vec::new();

        // Export files
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, path, lang, hash, lines, title, description FROM files ORDER BY project, path",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(FileEntry {
                    project: row.get(0)?,
                    path: row.get(1)?,
                    lang: row.get(2)?,
                    hash: row.get(3)?,
                    lines: row.get(4)?,
                    title: row.get(5)?,
                    description: row.get(6)?,
                })
            })?;
            for row in rows {
                files.push(row?);
            }
        }

        // Export symbols
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
                 FROM symbols
                 ORDER BY project, file, line_start",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(SymbolEntry {
                    project: row.get(0)?,
                    file: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: [row.get(4)?, row.get(5)?],
                    parent: row.get(6)?,
                    tokens: row.get(7)?,
                    alias: row.get(8)?,
                    visibility: row.get(9)?,
                })
            })?;
            for row in rows {
                symbols.push(row?);
            }
        }

        // Export texts
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, kind, line_start, line_end, text, parent
                 FROM texts
                 ORDER BY project, file, line_start",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(TextEntry {
                    project: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    line: [row.get(3)?, row.get(4)?],
                    text: row.get(5)?,
                    parent: row.get(6)?,
                })
            })?;
            for row in rows {
                texts.push(row?);
            }
        }

        // Export references
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, name, kind, line_start, line_end, caller
                 FROM refs
                 ORDER BY project, file, line_start",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ReferenceEntry {
                    project: row.get(0)?,
                    file: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: [row.get(4)?, row.get(5)?],
                    caller: row.get(6)?,
                })
            })?;
            for row in rows {
                references.push(row?);
            }
        }

        Ok((files, symbols, texts, references))
    }

    /// Export data for a specific project from DB back to vecs (for flushing to disk).
    #[allow(clippy::type_complexity)]
    pub fn export_for_project(
        &self,
        project: &str,
    ) -> Result<(
        Vec<FileEntry>,
        Vec<SymbolEntry>,
        Vec<TextEntry>,
        Vec<ReferenceEntry>,
    )> {
        let mut files = Vec::new();
        let mut symbols = Vec::new();
        let mut texts = Vec::new();
        let mut references = Vec::new();

        // Export files
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, path, lang, hash, lines, title, description FROM files WHERE project = ?1 ORDER BY path",
            )?;
            let rows = stmt.query_map([project], |row| {
                Ok(FileEntry {
                    project: row.get(0)?,
                    path: row.get(1)?,
                    lang: row.get(2)?,
                    hash: row.get(3)?,
                    lines: row.get(4)?,
                    title: row.get(5)?,
                    description: row.get(6)?,
                })
            })?;
            for row in rows {
                files.push(row?);
            }
        }

        // Export symbols
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
                 FROM symbols
                 WHERE project = ?1
                 ORDER BY file, line_start",
            )?;
            let rows = stmt.query_map([project], |row| {
                Ok(SymbolEntry {
                    project: row.get(0)?,
                    file: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: [row.get(4)?, row.get(5)?],
                    parent: row.get(6)?,
                    tokens: row.get(7)?,
                    alias: row.get(8)?,
                    visibility: row.get(9)?,
                })
            })?;
            for row in rows {
                symbols.push(row?);
            }
        }

        // Export texts
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, kind, line_start, line_end, text, parent
                 FROM texts
                 WHERE project = ?1
                 ORDER BY file, line_start",
            )?;
            let rows = stmt.query_map([project], |row| {
                Ok(TextEntry {
                    project: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    line: [row.get(3)?, row.get(4)?],
                    text: row.get(5)?,
                    parent: row.get(6)?,
                })
            })?;
            for row in rows {
                texts.push(row?);
            }
        }

        // Export references
        {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, name, kind, line_start, line_end, caller
                 FROM refs
                 WHERE project = ?1
                 ORDER BY file, line_start",
            )?;
            let rows = stmt.query_map([project], |row| {
                Ok(ReferenceEntry {
                    project: row.get(0)?,
                    file: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: [row.get(4)?, row.get(5)?],
                    caller: row.get(6)?,
                })
            })?;
            for row in rows {
                references.push(row?);
            }
        }

        Ok((files, symbols, texts, references))
    }

    /// List all unique projects in the database.
    pub fn list_projects(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT project FROM files ORDER BY project")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get directory overview: count of files per (parent_path, lang, min_visibility_level).
    ///
    /// Returns Vec of (parent_path, lang, min_visibility_level, count) tuples, sorted by parent_path.
    /// Skips dotfiles (paths starting with '.' or containing '/.').
    ///
    /// min_visibility_level is the minimum (most visible) visibility level of symbols in each file:
    /// - 0 = files with no symbols (documentation, configs, etc.)
    /// - 1 = files with public symbols
    /// - 2 = files with internal symbols (but no public)
    /// - 3 = files with only private symbols
    ///
    /// This allows callers to filter/display based on visibility without hiding files entirely.
    #[allow(clippy::type_complexity)]
    pub fn explore_dir_overview(
        &self,
        project: &str,
        path_prefix: Option<&str>,
    ) -> Result<Vec<(String, Option<String>, i32, usize)>> {
        let base = path_prefix
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty());

        // Query groups files by (parent_path, lang, min_visibility_level)
        // First compute each file's minimum visibility level, then group by that
        // Files without symbols get min_visibility_level = 0
        match base {
            Some(prefix) => {
                let glob = format!("{}/*", prefix);
                let mut stmt = self.conn.prepare(
                    "WITH file_min_vis AS (
                        SELECT f.parent_path, f.path, f.lang,
                               COALESCE(MIN(s.visibility_level), 3) as min_vis
                        FROM files f
                        LEFT JOIN symbols s ON s.project = f.project AND s.file = f.path
                        WHERE f.project = ?1 AND f.path GLOB ?2
                          AND f.path NOT GLOB '.*' AND f.path NOT GLOB '*/.*'
                        GROUP BY f.parent_path, f.path, f.lang
                    )
                    SELECT parent_path, lang, min_vis, COUNT(*) as cnt
                    FROM file_min_vis
                    GROUP BY parent_path, lang, min_vis
                    ORDER BY parent_path, lang, min_vis",
                )?;
                stmt.query_map(rusqlite::params![project, glob], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i32>(2)?,
                        row.get::<_, usize>(3)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("explore_dir_overview query failed")
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "WITH file_min_vis AS (
                        SELECT f.parent_path, f.path, f.lang,
                               COALESCE(MIN(s.visibility_level), 3) as min_vis
                        FROM files f
                        LEFT JOIN symbols s ON s.project = f.project AND s.file = f.path
                        WHERE f.project = ?1
                          AND f.path NOT GLOB '.*' AND f.path NOT GLOB '*/.*'
                        GROUP BY f.parent_path, f.path, f.lang
                    )
                    SELECT parent_path, lang, min_vis, COUNT(*) as cnt
                    FROM file_min_vis
                    GROUP BY parent_path, lang, min_vis
                    ORDER BY parent_path, lang, min_vis",
                )?;
                stmt.query_map(rusqlite::params![project], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i32>(2)?,
                        row.get::<_, usize>(3)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("explore_dir_overview query failed")
            }
        }
    }

    /// Get files in a specific directory.
    ///
    /// Returns Vec of (filename, lang) tuples, sorted by filename.
    pub fn explore_dir_files(
        &self,
        project: &str,
        parent_path: &str,
    ) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, lang FROM files
             WHERE project = ?1 AND parent_path = ?2
             AND path NOT GLOB '.*' AND path NOT GLOB '*/.*'
             ORDER BY path",
        )?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(rusqlite::params![project, parent_path], |row| {
                let path: String = row.get(0)?;
                let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
                Ok((filename, row.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Get files capped at N per (parent_path, lang), excluding markdown.
    ///
    /// Returns Vec of (parent_path, filename, lang) tuples.
    /// Uses ROW_NUMBER() to limit files per directory+language group.
    ///
    /// If visibility is specified, only returns files that contain symbols at that
    /// visibility level or higher.
    pub fn explore_files_capped(
        &self,
        project: &str,
        path_prefix: Option<&str>,
        visibility: Option<&str>,
        cap: usize,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let base = path_prefix
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty());

        // Cap to i64::MAX to avoid overflow when cap is usize::MAX
        let cap_i64 = cap.min(i64::MAX as usize) as i64;

        let max_level = visibility_max_level(visibility, "public");

        // Fetch files with known language (code + markdown)
        // Files with lang=NULL are summarized as "+N other files" from overview
        // When filtering by visibility:
        // - Files with no symbols have min_visibility_level = 3 (private, hidden by default)
        // - Files with symbols use the minimum visibility level of their symbols
        let rows: Vec<(String, String, Option<String>)> = match (max_level, base) {
            (Some(level), Some(prefix)) => {
                let glob = format!("{}/*", prefix);
                let mut stmt = self.conn.prepare(
                    "WITH file_visibility AS (
                        SELECT f.parent_path, f.path, f.lang,
                               COALESCE(MIN(s.visibility_level), 3) as min_vis
                        FROM files f
                        LEFT JOIN symbols s ON s.project = f.project AND s.file = f.path
                        WHERE f.project = ?1 AND f.path GLOB ?2
                          AND f.lang IS NOT NULL
                        GROUP BY f.parent_path, f.path, f.lang
                    ),
                    filtered_files AS (
                        SELECT parent_path, path, lang
                        FROM file_visibility
                        WHERE min_vis <= ?3
                    ),
                    ranked AS (
                        SELECT parent_path, path, lang,
                               ROW_NUMBER() OVER (PARTITION BY parent_path, lang ORDER BY path) as rn
                        FROM filtered_files
                    )
                    SELECT parent_path, path, lang FROM ranked WHERE rn <= ?4 ORDER BY parent_path, path",
                )?;
                stmt.query_map(rusqlite::params![project, glob, level, cap_i64], |row| {
                    let parent: String = row.get(0)?;
                    let path: String = row.get(1)?;
                    let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
                    Ok((parent, filename, row.get(2)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (Some(level), None) => {
                let mut stmt = self.conn.prepare(
                    "WITH file_visibility AS (
                        SELECT f.parent_path, f.path, f.lang,
                               COALESCE(MIN(s.visibility_level), 3) as min_vis
                        FROM files f
                        LEFT JOIN symbols s ON s.project = f.project AND s.file = f.path
                        WHERE f.project = ?1
                          AND f.lang IS NOT NULL
                        GROUP BY f.parent_path, f.path, f.lang
                    ),
                    filtered_files AS (
                        SELECT parent_path, path, lang
                        FROM file_visibility
                        WHERE min_vis <= ?2
                    ),
                    ranked AS (
                        SELECT parent_path, path, lang,
                               ROW_NUMBER() OVER (PARTITION BY parent_path, lang ORDER BY path) as rn
                        FROM filtered_files
                    )
                    SELECT parent_path, path, lang FROM ranked WHERE rn <= ?3 ORDER BY parent_path, path",
                )?;
                stmt.query_map(rusqlite::params![project, level, cap_i64], |row| {
                    let parent: String = row.get(0)?;
                    let path: String = row.get(1)?;
                    let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
                    Ok((parent, filename, row.get(2)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (None, Some(prefix)) => {
                let glob = format!("{}/*", prefix);
                let mut stmt = self.conn.prepare(
                    "WITH ranked AS (
                        SELECT parent_path, path, lang,
                               ROW_NUMBER() OVER (PARTITION BY parent_path, lang ORDER BY path) as rn
                        FROM files
                        WHERE project = ?1 AND path GLOB ?2
                          AND lang IS NOT NULL
                    )
                    SELECT parent_path, path, lang FROM ranked WHERE rn <= ?3 ORDER BY parent_path, path",
                )?;
                stmt.query_map(rusqlite::params![project, glob, cap_i64], |row| {
                    let parent: String = row.get(0)?;
                    let path: String = row.get(1)?;
                    let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
                    Ok((parent, filename, row.get(2)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (None, None) => {
                let mut stmt = self.conn.prepare(
                    "WITH ranked AS (
                        SELECT parent_path, path, lang,
                               ROW_NUMBER() OVER (PARTITION BY parent_path, lang ORDER BY path) as rn
                        FROM files
                        WHERE project = ?1
                          AND lang IS NOT NULL
                    )
                    SELECT parent_path, path, lang FROM ranked WHERE rn <= ?2 ORDER BY parent_path, path",
                )?;
                stmt.query_map(rusqlite::params![project, cap_i64], |row| {
                    let parent: String = row.get(0)?;
                    let path: String = row.get(1)?;
                    let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
                    Ok((parent, filename, row.get(2)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };

        Ok(rows)
    }
}

/// Pass through FTS5 query as-is.
///
/// The LLM is expected to emit valid FTS5 syntax directly.
/// See: https://www.sqlite.org/fts5.html#full_text_query_syntax
///
/// FTS5 syntax examples:
/// - `parseAsync` — single term
/// - `parseAsync OR safeParseAsync` — match either term
/// - `parseAsync AND safeParseAsync` — match both terms (implicit for space-separated)
/// - `parse*` — prefix search (matches parseAsync, parseString, etc.)
/// - `"safe parse"` — phrase search (exact sequence)
/// - `parse -test` — exclude results containing "test"
/// - `NEAR(parse async, 5)` — terms within 5 tokens of each other
fn fts5_quote(s: &str) -> String {
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db_with_refs(refs: &[ReferenceEntry]) -> SearchDb {
        let db = SearchDb::new_no_fts().unwrap();
        db.load("test", &[], &[], &[], refs).unwrap();
        db
    }

    #[test]
    fn test_get_callers_base_name_match() {
        // Insert a reference with a "self." prefixed name (as Python parser produces)
        let refs = vec![ReferenceEntry {
            project: String::new(),
            file: "app.py".to_string(),
            name: "self.handle_exception".to_string(),
            kind: "call".to_string(),
            line: [100, 100],
            caller: Some("full_dispatch_request".to_string()),
        }];
        let db = setup_test_db_with_refs(&refs);

        // Query with base name should find the reference
        // Use visibility="private" to skip symbol join (no symbols in test data)
        let results = db
            .get_callers(
                "handle_exception",
                None,
                Some("test"),
                Some("private"),
                100,
                0,
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "self.handle_exception");
        assert_eq!(results[0].caller.as_deref(), Some("full_dispatch_request"));
    }

    #[test]
    fn test_get_callers_exact_match_still_works() {
        // Insert a reference with full name
        let refs = vec![ReferenceEntry {
            project: String::new(),
            file: "app.py".to_string(),
            name: "self.handle_exception".to_string(),
            kind: "call".to_string(),
            line: [100, 100],
            caller: Some("dispatch".to_string()),
        }];
        let db = setup_test_db_with_refs(&refs);

        // Query with exact name should still work
        // Use visibility="private" to skip symbol join (no symbols in test data)
        let results = db
            .get_callers(
                "self.handle_exception",
                None,
                Some("test"),
                Some("private"),
                100,
                0,
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "self.handle_exception");
    }

    #[test]
    fn test_get_callers_no_false_positives() {
        // Insert references
        let refs = vec![
            ReferenceEntry {
                project: String::new(),
                file: "app.py".to_string(),
                name: "self.handle_exception".to_string(),
                kind: "call".to_string(),
                line: [100, 100],
                caller: None,
            },
            ReferenceEntry {
                project: String::new(),
                file: "app.py".to_string(),
                name: "handle_user_exception".to_string(),
                kind: "call".to_string(),
                line: [200, 200],
                caller: None,
            },
        ];
        let db = setup_test_db_with_refs(&refs);

        // Query for "handle_exception" should NOT match "handle_user_exception"
        // because it doesn't end with ".handle_exception"
        // Use visibility="private" to skip symbol join (no symbols in test data)
        let results = db
            .get_callers(
                "handle_exception",
                None,
                Some("test"),
                Some("private"),
                100,
                0,
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "self.handle_exception");
    }

    #[test]
    fn test_get_callees_base_name_match() {
        // Insert a reference with a qualified caller name
        let refs = vec![ReferenceEntry {
            project: String::new(),
            file: "app.py".to_string(),
            name: "os.path.join".to_string(),
            kind: "call".to_string(),
            line: [100, 100],
            caller: Some("MyClass.process_data".to_string()),
        }];
        let db = setup_test_db_with_refs(&refs);

        // Query with base caller name should find the reference
        // Use visibility="private" to skip symbol join (no symbols in test data)
        let results = db
            .get_callees("process_data", None, Some("test"), Some("private"), 100, 0)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "os.path.join");
        assert_eq!(results[0].caller.as_deref(), Some("MyClass.process_data"));
    }

    #[test]
    fn test_get_callees_exact_match_still_works() {
        // Insert a reference
        let refs = vec![ReferenceEntry {
            project: String::new(),
            file: "app.py".to_string(),
            name: "helper".to_string(),
            kind: "call".to_string(),
            line: [100, 100],
            caller: Some("MyClass.method".to_string()),
        }];
        let db = setup_test_db_with_refs(&refs);

        // Query with exact caller name should work
        // Use visibility="private" to skip symbol join (no symbols in test data)
        let results = db
            .get_callees(
                "MyClass.method",
                None,
                Some("test"),
                Some("private"),
                100,
                0,
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "helper");
    }

    // Visibility filter tests

    fn setup_test_db_with_symbols(symbols: &[SymbolEntry]) -> SearchDb {
        let db = SearchDb::new_no_fts().unwrap();
        // Collect unique files by path
        let mut seen = std::collections::HashSet::new();
        let files: Vec<FileEntry> = symbols
            .iter()
            .filter(|s| seen.insert(s.file.clone()))
            .map(|s| FileEntry {
                project: s.project.clone(),
                path: s.file.clone(),
                lang: Some("rust".to_string()),
                hash: "abc123".to_string(),
                lines: 100,
                title: None,
                description: None,
            })
            .collect();
        db.load("test", &files, symbols, &[], &[]).unwrap();
        db
    }

    #[test]
    fn test_visibility_max_level_function() {
        // Test the visibility_max_level helper function with explicit values
        assert_eq!(visibility_max_level(Some("public"), "public"), Some(1));
        assert_eq!(visibility_max_level(Some("internal"), "public"), Some(2));
        assert_eq!(visibility_max_level(Some("private"), "public"), None);
        assert_eq!(visibility_max_level(Some("unknown"), "public"), None); // Unknown, no filter

        // Test default behavior (None uses the default parameter)
        assert_eq!(visibility_max_level(None, "public"), Some(1)); // Default: public
        assert_eq!(visibility_max_level(None, "internal"), Some(2)); // Default: internal
        assert_eq!(visibility_max_level(None, "private"), None); // Default: private (no filter)
    }

    #[test]
    fn test_visibility_to_level_function() {
        // Test the visibility_to_level helper function
        assert_eq!(visibility_to_level(Some("public")), 1);
        assert_eq!(visibility_to_level(Some("internal")), 2);
        assert_eq!(visibility_to_level(Some("private")), 3);
        assert_eq!(visibility_to_level(None), 3);
    }

    #[test]
    fn test_get_file_symbols_visibility_filter() {
        let symbols = vec![
            SymbolEntry {
                project: "test".to_string(),
                file: "lib.rs".to_string(),
                name: "public_fn".to_string(),
                kind: "function".to_string(),
                line: [10, 20],
                parent: None,
                tokens: None,
                alias: None,
                visibility: Some("public".to_string()),
            },
            SymbolEntry {
                project: "test".to_string(),
                file: "lib.rs".to_string(),
                name: "internal_fn".to_string(),
                kind: "function".to_string(),
                line: [30, 40],
                parent: None,
                tokens: None,
                alias: None,
                visibility: Some("internal".to_string()),
            },
            SymbolEntry {
                project: "test".to_string(),
                file: "lib.rs".to_string(),
                name: "private_fn".to_string(),
                kind: "function".to_string(),
                line: [50, 60],
                parent: None,
                tokens: None,
                alias: None,
                visibility: Some("private".to_string()),
            },
        ];
        let db = setup_test_db_with_symbols(&symbols);

        // Default (None) = public - returns only public
        let results = db.get_file_symbols("lib.rs", None, 100, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "public_fn");

        // Explicit public - same as default
        let results = db
            .get_file_symbols("lib.rs", Some("public"), 100, 0)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "public_fn");

        // Internal filter - returns public and internal
        let results = db
            .get_file_symbols("lib.rs", Some("internal"), 100, 0)
            .unwrap();
        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"public_fn"));
        assert!(names.contains(&"internal_fn"));

        // Private filter - returns all
        let results = db
            .get_file_symbols("lib.rs", Some("private"), 100, 0)
            .unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_get_children_visibility_filter() {
        let symbols = vec![
            SymbolEntry {
                project: "test".to_string(),
                file: "lib.rs".to_string(),
                name: "MyStruct".to_string(),
                kind: "struct".to_string(),
                line: [1, 50],
                parent: None,
                tokens: None,
                alias: None,
                visibility: Some("public".to_string()),
            },
            SymbolEntry {
                project: "test".to_string(),
                file: "lib.rs".to_string(),
                name: "public_method".to_string(),
                kind: "method".to_string(),
                line: [10, 15],
                parent: Some("MyStruct".to_string()),
                tokens: None,
                alias: None,
                visibility: Some("public".to_string()),
            },
            SymbolEntry {
                project: "test".to_string(),
                file: "lib.rs".to_string(),
                name: "private_method".to_string(),
                kind: "method".to_string(),
                line: [20, 25],
                parent: Some("MyStruct".to_string()),
                tokens: None,
                alias: None,
                visibility: Some("private".to_string()),
            },
        ];
        let db = setup_test_db_with_symbols(&symbols);

        // Default (None) = public - returns only public method
        let results = db.get_children("lib.rs", "MyStruct", None, 100, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "public_method");

        // Private filter - returns all methods
        let results = db
            .get_children("lib.rs", "MyStruct", Some("private"), 100, 0)
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_explore_dir_overview_files_with_no_symbols() {
        // Test that files with no symbols get min_visibility_level = 3 (private)
        // They should be hidden by default, only shown with visibility=private
        let db = SearchDb::new_no_fts().unwrap();

        // Create files: one with symbols, one without (like empty __init__.py)
        let files = vec![
            FileEntry {
                project: "test".to_string(),
                path: "pkg/__init__.py".to_string(),
                lang: Some("python".to_string()),
                hash: "abc".to_string(),
                lines: 0, // Empty file
                title: None,
                description: None,
            },
            FileEntry {
                project: "test".to_string(),
                path: "pkg/module.py".to_string(),
                lang: Some("python".to_string()),
                hash: "def".to_string(),
                lines: 50,
                title: None,
                description: None,
            },
        ];

        // Only module.py has symbols (public function)
        let symbols = vec![SymbolEntry {
            project: "test".to_string(),
            file: "pkg/module.py".to_string(),
            name: "public_func".to_string(),
            kind: "function".to_string(),
            line: [1, 10],
            parent: None,
            tokens: None,
            alias: None,
            visibility: Some("public".to_string()),
        }];

        db.load("test", &files, &symbols, &[], &[]).unwrap();

        // Get overview - should return separate entries for each file
        let overview = db.explore_dir_overview("test", None).unwrap();

        // Should have at least 2 entries: one for __init__.py (level 3), one for module.py (level 1)
        assert!(!overview.is_empty(), "Overview should not be empty");

        // Find entries for pkg directory
        let pkg_entries: Vec<_> = overview
            .iter()
            .filter(|(dir, _, _, _)| dir == "pkg")
            .collect();

        // Check that we have entries with different visibility levels
        let has_level_3 = pkg_entries.iter().any(|(_, _, vis, _)| *vis == 3);
        let has_level_1 = pkg_entries.iter().any(|(_, _, vis, _)| *vis == 1);

        assert!(
            has_level_3,
            "Should have entry for file with no symbols (visibility_level=3, private)"
        );
        assert!(
            has_level_1,
            "Should have entry for file with public symbol (visibility_level=1)"
        );
    }

    #[test]
    fn test_explore_files_capped_with_visibility_filter() {
        // Test that explore_files_capped filters by visibility correctly
        let db = SearchDb::new_no_fts().unwrap();

        let files = vec![
            FileEntry {
                project: "test".to_string(),
                path: "pkg/__init__.py".to_string(),
                lang: Some("python".to_string()),
                hash: "abc".to_string(),
                lines: 0,
                title: None,
                description: None,
            },
            FileEntry {
                project: "test".to_string(),
                path: "pkg/public_mod.py".to_string(),
                lang: Some("python".to_string()),
                hash: "def".to_string(),
                lines: 50,
                title: None,
                description: None,
            },
            FileEntry {
                project: "test".to_string(),
                path: "pkg/private_mod.py".to_string(),
                lang: Some("python".to_string()),
                hash: "ghi".to_string(),
                lines: 30,
                title: None,
                description: None,
            },
        ];

        let symbols = vec![
            SymbolEntry {
                project: "test".to_string(),
                file: "pkg/public_mod.py".to_string(),
                name: "public_func".to_string(),
                kind: "function".to_string(),
                line: [1, 10],
                parent: None,
                tokens: None,
                alias: None,
                visibility: Some("public".to_string()),
            },
            SymbolEntry {
                project: "test".to_string(),
                file: "pkg/private_mod.py".to_string(),
                name: "_private_func".to_string(),
                kind: "function".to_string(),
                line: [1, 10],
                parent: None,
                tokens: None,
                alias: None,
                visibility: Some("private".to_string()),
            },
        ];

        db.load("test", &files, &symbols, &[], &[]).unwrap();

        // Default (None) = public - returns:
        // - Files with public symbols (min_vis = 1): public_mod.py
        // Files with no symbols (__init__.py) have min_vis = 3 (private), so they're hidden
        let default_files = db.explore_files_capped("test", None, None, 100).unwrap();
        assert_eq!(default_files.len(), 1);
        let filenames: Vec<&str> = default_files.iter().map(|f| f.1.as_str()).collect();
        assert!(filenames.contains(&"public_mod.py")); // has public symbol
        assert!(!filenames.contains(&"__init__.py")); // no symbols, hidden by default

        // Explicit public - same as default
        let public_files = db
            .explore_files_capped("test", None, Some("public"), 100)
            .unwrap();
        assert_eq!(public_files.len(), 1);

        // Internal visibility filter - returns:
        // - Files with public symbols (min_vis = 1): public_mod.py
        // - __init__.py has level 3 (no symbols), so it's excluded
        // - private_mod.py has level 3 (private symbol), so it's also excluded
        let internal_files = db
            .explore_files_capped("test", None, Some("internal"), 100)
            .unwrap();
        assert_eq!(internal_files.len(), 1);

        // Private visibility filter - returns ALL files with known language
        let all_files = db
            .explore_files_capped("test", None, Some("private"), 100)
            .unwrap();
        assert_eq!(all_files.len(), 3);
    }
}
