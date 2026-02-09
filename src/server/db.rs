use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::index::format::{FileEntry, ReferenceEntry, SymbolEntry, TextEntry};

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
                visibility TEXT
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
        // Replaces separate files_fts, symbols_fts, texts_fts tables
        if fts_enabled {
            conn.execute_batch(
                "
                CREATE VIRTUAL TABLE search_fts USING fts5(
                    content,            -- searchable text (type-specific, includes path)
                    type UNINDEXED,     -- 'symbol', 'file', 'text'
                    rowid_ref UNINDEXED,-- rowid in source table
                    path UNINDEXED,     -- file path (for GLOB filtering)
                    kind UNINDEXED,     -- symbol/text kind, or file lang
                    project UNINDEXED   -- project filter
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
                "INSERT INTO symbols (project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        if self.fts_enabled {
            tx.execute_batch(
                "
                -- Insert files: content = path + title + description
                INSERT INTO search_fts(content, type, rowid_ref, path, kind, project)
                SELECT
                    COALESCE(path, '') || ' ' || COALESCE(title, '') || ' ' || COALESCE(description, ''),
                    'file',
                    rowid,
                    path,
                    lang,
                    project
                FROM files;

                -- Insert symbols: content = file + name + tokens
                INSERT INTO search_fts(content, type, rowid_ref, path, kind, project)
                SELECT
                    COALESCE(file, '') || ' ' || COALESCE(name, '') || ' ' || COALESCE(tokens, ''),
                    'symbol',
                    rowid,
                    file,
                    kind,
                    project
                FROM symbols;

                -- Insert texts: content = file + text
                INSERT INTO search_fts(content, type, rowid_ref, path, kind, project)
                SELECT
                    COALESCE(file, '') || ' ' || COALESCE(text, ''),
                    'text',
                    rowid,
                    file,
                    kind,
                    project
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
    /// - limit: Max results (default 100)
    /// - offset: Pagination offset
    ///
    /// Returns results ordered by BM25 relevance.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        query: &str,
        scope: &[String],
        kind: Option<&str>,
        path: Option<&str>,
        project: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SearchResult>> {
        // Build FTS5 MATCH expression
        let fts_query = format!("content : {}", fts5_quote(query));

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

        let next_param = params.len() + 1;

        // Kind filter
        if let Some(k) = kind {
            conditions.push(format!("kind = ?{}", next_param));
            params.push(Box::new(k.to_string()));
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

        // Add limit and offset
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        let sql = format!(
            "SELECT type, rowid_ref FROM search_fts WHERE {} ORDER BY rank LIMIT ?{} OFFSET ?{}",
            conditions.join(" AND "),
            next_param,
            next_param + 1
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
                "symbol" => self.get_symbol_by_rowid(rowid).map(SearchResult::Symbol),
                "file" => self.get_file_by_rowid(rowid).map(SearchResult::File),
                "text" => self.get_text_by_rowid(rowid).map(SearchResult::Text),
                _ => continue,
            };
            if let Ok(r) = result {
                results.push(r);
            }
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
    pub fn get_file_symbols(&self, file: &str) -> Result<Vec<SymbolEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
             FROM symbols
             WHERE file = ?1
             ORDER BY line_start",
        )?;

        let rows = stmt.query_map([file], |row| {
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

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get direct children of a symbol in a file.
    pub fn get_symbol_children(&self, file: &str, parent: &str) -> Result<Vec<SymbolEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
             FROM symbols
             WHERE file = ?1 AND parent = ?2
             ORDER BY line_start",
        )?;

        let rows = stmt.query_map(rusqlite::params![file, parent], |row| {
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

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all imports for a file (symbols with kind "import").
    pub fn get_imports(&self, file: &str) -> Result<Vec<SymbolEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility
             FROM symbols
             WHERE file = ?1 AND kind = 'import'
             ORDER BY line_start",
        )?;

        let rows = stmt.query_map([file], |row| {
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

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all references TO a symbol (who calls/uses this symbol).
    /// Returns references sorted by file, then line.
    pub fn get_callers(
        &self,
        name: &str,
        kind: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<ReferenceEntry>> {
        let mut conditions = vec!["name = ?".to_string()];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(name.to_string())];

        if let Some(k) = kind {
            conditions.push("kind = ?".to_string());
            params.push(Box::new(k.to_string()));
        }
        if let Some(p) = project {
            conditions.push("project = ?".to_string());
            params.push(Box::new(p.to_string()));
        }

        let sql = format!(
            "SELECT project, file, name, kind, line_start, line_end, caller
             FROM refs
             WHERE {}
             ORDER BY file, line_start",
            conditions.join(" AND ")
        );

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
    pub fn get_callees(
        &self,
        caller: &str,
        kind: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<ReferenceEntry>> {
        let mut conditions = vec!["caller = ?".to_string()];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(caller.to_string())];

        if let Some(k) = kind {
            conditions.push("kind = ?".to_string());
            params.push(Box::new(k.to_string()));
        }
        if let Some(p) = project {
            conditions.push("project = ?".to_string());
            params.push(Box::new(p.to_string()));
        }

        let sql = format!(
            "SELECT project, file, name, kind, line_start, line_end, caller
             FROM refs
             WHERE {}
             ORDER BY file, line_start",
            conditions.join(" AND ")
        );

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
                "INSERT INTO symbols (project, file, name, kind, line_start, line_end, parent, tokens, alias, visibility)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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

            -- Insert files: content = path + title + description
            INSERT INTO search_fts(content, type, rowid_ref, path, kind, project)
            SELECT
                COALESCE(path, '') || ' ' || COALESCE(title, '') || ' ' || COALESCE(description, ''),
                'file',
                rowid,
                path,
                lang,
                project
            FROM files;

            -- Insert symbols: content = file + name + tokens
            INSERT INTO search_fts(content, type, rowid_ref, path, kind, project)
            SELECT
                COALESCE(file, '') || ' ' || COALESCE(name, '') || ' ' || COALESCE(tokens, ''),
                'symbol',
                rowid,
                file,
                kind,
                project
            FROM symbols;

            -- Insert texts: content = file + text
            INSERT INTO search_fts(content, type, rowid_ref, path, kind, project)
            SELECT
                COALESCE(file, '') || ' ' || COALESCE(text, ''),
                'text',
                rowid,
                file,
                kind,
                project
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

    /// Get directory overview: count of files per (parent_path, lang).
    ///
    /// Returns Vec of (parent_path, lang, count) tuples, sorted by parent_path.
    /// Skips dotfiles (paths starting with '.' or containing '/.').
    pub fn explore_dir_overview(
        &self,
        project: &str,
        path_prefix: Option<&str>,
    ) -> Result<Vec<(String, Option<String>, usize)>> {
        let base = path_prefix
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty());

        let rows: Vec<(String, Option<String>, usize)> = match base {
            Some(prefix) => {
                let glob = format!("{}/*", prefix);
                let mut stmt = self.conn.prepare(
                    "SELECT parent_path, lang, COUNT(*) as cnt FROM files
                     WHERE project = ?1 AND path GLOB ?2
                     AND path NOT GLOB '.*' AND path NOT GLOB '*/.*'
                     GROUP BY parent_path, lang ORDER BY parent_path, lang",
                )?;
                stmt.query_map(rusqlite::params![project, glob], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, usize>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT parent_path, lang, COUNT(*) as cnt FROM files
                     WHERE project = ?1
                     AND path NOT GLOB '.*' AND path NOT GLOB '*/.*'
                     GROUP BY parent_path, lang ORDER BY parent_path, lang",
                )?;
                stmt.query_map(rusqlite::params![project], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, usize>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };

        Ok(rows)
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
    pub fn explore_files_capped(
        &self,
        project: &str,
        path_prefix: Option<&str>,
        cap: usize,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let base = path_prefix
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty());

        // Cap to i64::MAX to avoid overflow when cap is usize::MAX
        let cap_i64 = cap.min(i64::MAX as usize) as i64;

        // Fetch files with known language (code + markdown)
        // Files with lang=NULL are summarized as "+N other files" from overview
        let rows: Vec<(String, String, Option<String>)> = match base {
            Some(prefix) => {
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
            None => {
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
