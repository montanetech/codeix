use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::index::format::{FileEntry, ReferenceEntry, SymbolEntry, TextEntry};

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
                lang        TEXT,
                hash        TEXT NOT NULL,
                lines       INTEGER NOT NULL,
                title       TEXT,
                description TEXT,
                PRIMARY KEY (project, path)
            );

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

        // FTS5 virtual tables for full-text search (only when enabled)
        if fts_enabled {
            conn.execute_batch(
                "
                CREATE VIRTUAL TABLE files_fts USING fts5(
                    project,
                    path,
                    lang,
                    title,
                    description,
                    content='files',
                    content_rowid='rowid'
                );

                CREATE VIRTUAL TABLE symbols_fts USING fts5(
                    project,
                    name,
                    tokens,
                    file,
                    kind,
                    content='symbols',
                    content_rowid='rowid'
                );

                CREATE VIRTUAL TABLE texts_fts USING fts5(
                    project,
                    text,
                    file,
                    kind,
                    content='texts',
                    content_rowid='rowid'
                );

                CREATE VIRTUAL TABLE refs_fts USING fts5(
                    project,
                    name,
                    kind,
                    file,
                    caller,
                    content='refs',
                    content_rowid='rowid'
                );
                ",
            )
            .context("failed to create FTS5 tables")?;
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
                "INSERT INTO files (project, path, lang, hash, lines, title, description) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for f in files {
                stmt.execute(rusqlite::params![
                    project,
                    f.path,
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

        // Populate FTS5 indexes from content tables (only when FTS enabled)
        if self.fts_enabled {
            tx.execute_batch(
                "
                INSERT INTO files_fts(files_fts) VALUES('rebuild');
                INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');
                INSERT INTO texts_fts(texts_fts) VALUES('rebuild');
                INSERT INTO refs_fts(refs_fts) VALUES('rebuild');
                ",
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Search or list symbols.
    /// - With query (no glob in file): FTS5 full-text search on symbol names (BM25-ranked)
    /// - Without query OR with glob file pattern: List matching symbols (ordered by file, line)
    ///
    /// File filter supports glob patterns with * (e.g. "src/*.py").
    /// Note: When file contains glob patterns, uses SQL GLOB for filtering (case-sensitive).
    pub fn search_symbols(
        &self,
        query: Option<&str>,
        kind: Option<&str>,
        file: Option<&str>,
        project: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SymbolEntry>> {
        // Check if file filter contains glob pattern
        let file_has_glob = file.map(|f| f.contains('*')).unwrap_or(false);

        match query {
            // Search mode: use FTS5 with BM25 ranking
            Some(q) if !q.is_empty() && !file_has_glob => {
                self.search_symbols_fts(q, kind, file, project, limit, offset)
            }
            // List mode: plain SQL query (when no query or file has glob pattern)
            _ => self.list_symbols(query, kind, file, project, limit, offset),
        }
    }

    /// FTS5 search on symbol names (BM25-ranked).
    fn search_symbols_fts(
        &self,
        query: &str,
        kind: Option<&str>,
        file: Option<&str>,
        project: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SymbolEntry>> {
        // Build the FTS5 match expression, incorporating filters into the query
        let mut fts_parts = vec![format!("name : {}", fts5_quote(query))];
        if let Some(k) = kind {
            fts_parts.push(format!("kind : {}", fts5_quote(k)));
        }
        if let Some(f) = file {
            fts_parts.push(format!("file : {}", fts5_quote(f)));
        }
        if let Some(p) = project {
            fts_parts.push(format!("project : {}", fts5_quote(p)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT s.project, s.file, s.name, s.kind, s.line_start, s.line_end,
                    s.parent, s.tokens, s.alias, s.visibility
             FROM symbols_fts f
             JOIN symbols s ON s.rowid = f.rowid
             WHERE symbols_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2 OFFSET ?3",
        )?;

        let rows = stmt.query_map(rusqlite::params![&fts_query, limit, offset], |row| {
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

    /// List symbols matching filters (no FTS, ordered by file and line).
    /// File filter supports SQLite GLOB patterns: * matches any sequence, ? matches single char.
    /// Query performs substring match on symbol name when provided.
    fn list_symbols(
        &self,
        query: Option<&str>,
        kind: Option<&str>,
        file: Option<&str>,
        project: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SymbolEntry>> {
        // Build WHERE clause dynamically
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(q) = query
            && !q.is_empty()
        {
            // Simple substring match on name when in list mode with glob file
            conditions.push("name LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", q)));
        }
        if let Some(k) = kind {
            conditions.push("kind = ?".to_string());
            params.push(Box::new(k.to_string()));
        }
        if let Some(f) = file {
            if f.contains('*') {
                // Use GLOB for pattern matching (supports * wildcard)
                conditions.push("file GLOB ?".to_string());
                params.push(Box::new(f.to_string()));
            } else {
                conditions.push("file = ?".to_string());
                params.push(Box::new(f.to_string()));
            }
        }
        if let Some(p) = project {
            conditions.push("project = ?".to_string());
            params.push(Box::new(p.to_string()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT project, file, name, kind, line_start, line_end,
                    parent, tokens, alias, visibility
             FROM symbols
             {}
             ORDER BY file, line_start
             LIMIT ? OFFSET ?",
            where_clause
        );

        let mut stmt = self.conn.prepare(&sql)?;

        // Build params slice for query
        let mut param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        param_refs.push(&limit);
        param_refs.push(&offset);

        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs), |row| {
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

    /// FTS5 search on text content, with optional kind, file, and project filters.
    pub fn search_text(
        &self,
        query: &str,
        kind: Option<&str>,
        file: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<TextEntry>> {
        let mut fts_parts = vec![format!("text : {}", fts5_quote(query))];
        if let Some(k) = kind {
            fts_parts.push(format!("kind : {}", fts5_quote(k)));
        }
        if let Some(f) = file {
            fts_parts.push(format!("file : {}", fts5_quote(f)));
        }
        if let Some(p) = project {
            fts_parts.push(format!("project : {}", fts5_quote(p)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT t.project, t.file, t.kind, t.line_start, t.line_end, t.text, t.parent
             FROM texts_fts f
             JOIN texts t ON t.rowid = f.rowid
             WHERE texts_fts MATCH ?1
             ORDER BY rank
             LIMIT 100",
        )?;

        let rows = stmt.query_map([&fts_query], |row| {
            Ok(TextEntry {
                project: row.get(0)?,
                file: row.get(1)?,
                kind: row.get(2)?,
                line: [row.get(3)?, row.get(4)?],
                text: row.get(5)?,
                parent: row.get(6)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// FTS5 search on file paths, titles, and descriptions, with optional language and project filters.
    pub fn search_files(
        &self,
        query: &str,
        lang: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<FileEntry>> {
        // Search across path, title, and description fields
        let mut fts_parts = vec![format!(
            "(path : {} OR title : {} OR description : {})",
            fts5_quote(query),
            fts5_quote(query),
            fts5_quote(query)
        )];
        if let Some(l) = lang {
            fts_parts.push(format!("lang : {}", fts5_quote(l)));
        }
        if let Some(p) = project {
            fts_parts.push(format!("project : {}", fts5_quote(p)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT fl.project, fl.path, fl.lang, fl.hash, fl.lines, fl.title, fl.description
             FROM files_fts f
             JOIN files fl ON fl.rowid = f.rowid
             WHERE files_fts MATCH ?1
             ORDER BY rank
             LIMIT 100",
        )?;

        let rows = stmt.query_map([&fts_query], |row| {
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

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
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

    /// Search references by name using FTS5 (BM25-ranked).
    pub fn search_references(
        &self,
        query: &str,
        kind: Option<&str>,
        file: Option<&str>,
        project: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ReferenceEntry>> {
        let mut fts_parts = vec![format!("name : {}", fts5_quote(query))];
        if let Some(k) = kind {
            fts_parts.push(format!("kind : {}", fts5_quote(k)));
        }
        if let Some(f) = file {
            fts_parts.push(format!("file : {}", fts5_quote(f)));
        }
        if let Some(p) = project {
            fts_parts.push(format!("project : {}", fts5_quote(p)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT r.project, r.file, r.name, r.kind, r.line_start, r.line_end, r.caller
             FROM refs_fts f
             JOIN refs r ON r.rowid = f.rowid
             WHERE refs_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2 OFFSET ?3",
        )?;

        let rows = stmt.query_map(rusqlite::params![&fts_query, limit, offset], |row| {
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
        tx.execute(
            "INSERT INTO files (project, path, lang, hash, lines, title, description) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![project, file.path, file.lang, file.hash, file.lines, file.title, file.description],
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

    /// Rebuild all FTS5 indexes.
    /// Call this after batch upsert/remove operations.
    /// No-op when FTS is disabled (build mode).
    pub fn rebuild_fts(&self) -> Result<()> {
        if !self.fts_enabled {
            return Ok(());
        }
        self.conn.execute_batch(
            "
            INSERT INTO files_fts(files_fts) VALUES('rebuild');
            INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');
            INSERT INTO texts_fts(texts_fts) VALUES('rebuild');
            INSERT INTO refs_fts(refs_fts) VALUES('rebuild');
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
}

/// Quote a string for use in an FTS5 MATCH expression.
/// Wraps in double quotes and escapes any internal double quotes.
fn fts5_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
