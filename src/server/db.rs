use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::index::format::{FileEntry, SymbolEntry, TextEntry};

/// An in-memory SQLite database with FTS5 virtual tables for fast text search
/// over the code index.
pub struct SearchDb {
    conn: Connection,
}

impl SearchDb {
    /// Create a new in-memory database and initialize the FTS5 schema.
    pub fn new() -> Result<Self> {
        let conn = Connection::open_in_memory()?;

        // Content tables (store the actual data for retrieval)
        conn.execute_batch(
            "
            CREATE TABLE files (
                path    TEXT PRIMARY KEY,
                lang    TEXT,
                hash    TEXT NOT NULL,
                lines   INTEGER NOT NULL
            );

            CREATE TABLE symbols (
                file       TEXT NOT NULL,
                name       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end   INTEGER NOT NULL,
                parent     TEXT,
                sig        TEXT,
                alias      TEXT,
                visibility TEXT
            );

            CREATE TABLE texts (
                file       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end   INTEGER NOT NULL,
                text       TEXT NOT NULL,
                parent     TEXT
            );

            -- FTS5 virtual tables for full-text search
            CREATE VIRTUAL TABLE files_fts USING fts5(
                path,
                lang,
                content='files',
                content_rowid='rowid'
            );

            CREATE VIRTUAL TABLE symbols_fts USING fts5(
                name,
                file,
                kind,
                content='symbols',
                content_rowid='rowid'
            );

            CREATE VIRTUAL TABLE texts_fts USING fts5(
                text,
                file,
                kind,
                content='texts',
                content_rowid='rowid'
            );

            -- Indexes for exact lookups
            CREATE INDEX idx_symbols_file ON symbols(file);
            CREATE INDEX idx_symbols_file_parent ON symbols(file, parent);
            CREATE INDEX idx_symbols_file_kind ON symbols(file, kind);
            CREATE INDEX idx_texts_file ON texts(file);
            ",
        )
        .context("failed to create database schema")?;

        Ok(Self { conn })
    }

    /// Load index data into the database.
    pub fn load(
        &self,
        files: &[FileEntry],
        symbols: &[SymbolEntry],
        texts: &[TextEntry],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Insert files
        {
            let mut stmt =
                tx.prepare("INSERT INTO files (path, lang, hash, lines) VALUES (?1, ?2, ?3, ?4)")?;
            for f in files {
                stmt.execute(rusqlite::params![f.path, f.lang, f.hash, f.lines])?;
            }
        }

        // Insert symbols
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols (file, name, kind, line_start, line_end, parent, sig, alias, visibility)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for s in symbols {
                stmt.execute(rusqlite::params![
                    s.file,
                    s.name,
                    s.kind,
                    s.line[0],
                    s.line[1],
                    s.parent,
                    s.sig,
                    s.alias,
                    s.visibility,
                ])?;
            }
        }

        // Insert texts
        {
            let mut stmt = tx.prepare(
                "INSERT INTO texts (file, kind, line_start, line_end, text, parent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for t in texts {
                stmt.execute(rusqlite::params![
                    t.file, t.kind, t.line[0], t.line[1], t.text, t.parent,
                ])?;
            }
        }

        // Populate FTS5 indexes from content tables
        tx.execute_batch(
            "
            INSERT INTO files_fts(files_fts) VALUES('rebuild');
            INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');
            INSERT INTO texts_fts(texts_fts) VALUES('rebuild');
            ",
        )?;

        tx.commit()?;
        Ok(())
    }

    /// FTS5 search on symbol names, with optional kind and file filters.
    pub fn search_symbols(
        &self,
        query: &str,
        kind: Option<&str>,
        file: Option<&str>,
    ) -> Result<Vec<SymbolEntry>> {
        // Build the FTS5 match expression, incorporating filters into the query
        let mut fts_parts = vec![format!("name : {}", fts5_quote(query))];
        if let Some(k) = kind {
            fts_parts.push(format!("kind : {}", fts5_quote(k)));
        }
        if let Some(f) = file {
            fts_parts.push(format!("file : {}", fts5_quote(f)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT s.file, s.name, s.kind, s.line_start, s.line_end,
                    s.parent, s.sig, s.alias, s.visibility
             FROM symbols_fts f
             JOIN symbols s ON s.rowid = f.rowid
             WHERE symbols_fts MATCH ?1
             ORDER BY rank
             LIMIT 100",
        )?;

        let rows = stmt.query_map([&fts_query], |row| {
            Ok(SymbolEntry {
                file: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: [row.get(3)?, row.get(4)?],
                parent: row.get(5)?,
                sig: row.get(6)?,
                alias: row.get(7)?,
                visibility: row.get(8)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// FTS5 search on text content, with optional kind and file filters.
    pub fn search_text(
        &self,
        query: &str,
        kind: Option<&str>,
        file: Option<&str>,
    ) -> Result<Vec<TextEntry>> {
        let mut fts_parts = vec![format!("text : {}", fts5_quote(query))];
        if let Some(k) = kind {
            fts_parts.push(format!("kind : {}", fts5_quote(k)));
        }
        if let Some(f) = file {
            fts_parts.push(format!("file : {}", fts5_quote(f)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT t.file, t.kind, t.line_start, t.line_end, t.text, t.parent
             FROM texts_fts f
             JOIN texts t ON t.rowid = f.rowid
             WHERE texts_fts MATCH ?1
             ORDER BY rank
             LIMIT 100",
        )?;

        let rows = stmt.query_map([&fts_query], |row| {
            Ok(TextEntry {
                file: row.get(0)?,
                kind: row.get(1)?,
                line: [row.get(2)?, row.get(3)?],
                text: row.get(4)?,
                parent: row.get(5)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// FTS5 search on file paths, with optional language filter.
    pub fn search_files(&self, query: &str, lang: Option<&str>) -> Result<Vec<FileEntry>> {
        let mut fts_parts = vec![format!("path : {}", fts5_quote(query))];
        if let Some(l) = lang {
            fts_parts.push(format!("lang : {}", fts5_quote(l)));
        }
        let fts_query = fts_parts.join(" AND ");

        let mut stmt = self.conn.prepare(
            "SELECT fl.path, fl.lang, fl.hash, fl.lines
             FROM files_fts f
             JOIN files fl ON fl.rowid = f.rowid
             WHERE files_fts MATCH ?1
             ORDER BY rank
             LIMIT 100",
        )?;

        let rows = stmt.query_map([&fts_query], |row| {
            Ok(FileEntry {
                path: row.get(0)?,
                lang: row.get(1)?,
                hash: row.get(2)?,
                lines: row.get(3)?,
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
            "SELECT file, name, kind, line_start, line_end, parent, sig, alias, visibility
             FROM symbols
             WHERE file = ?1
             ORDER BY line_start",
        )?;

        let rows = stmt.query_map([file], |row| {
            Ok(SymbolEntry {
                file: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: [row.get(3)?, row.get(4)?],
                parent: row.get(5)?,
                sig: row.get(6)?,
                alias: row.get(7)?,
                visibility: row.get(8)?,
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
            "SELECT file, name, kind, line_start, line_end, parent, sig, alias, visibility
             FROM symbols
             WHERE file = ?1 AND parent = ?2
             ORDER BY line_start",
        )?;

        let rows = stmt.query_map(rusqlite::params![file, parent], |row| {
            Ok(SymbolEntry {
                file: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: [row.get(3)?, row.get(4)?],
                parent: row.get(5)?,
                sig: row.get(6)?,
                alias: row.get(7)?,
                visibility: row.get(8)?,
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
            "SELECT file, name, kind, line_start, line_end, parent, sig, alias, visibility
             FROM symbols
             WHERE file = ?1 AND kind = 'import'
             ORDER BY line_start",
        )?;

        let rows = stmt.query_map([file], |row| {
            Ok(SymbolEntry {
                file: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: [row.get(3)?, row.get(4)?],
                parent: row.get(5)?,
                sig: row.get(6)?,
                alias: row.get(7)?,
                visibility: row.get(8)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get the hash of a file from the DB (for change detection).
    pub fn get_file_hash(&self, path: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT hash FROM files WHERE path = ?1")?;
        let mut rows = stmt.query([path])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Remove all data for a file (from files, symbols, texts tables).
    /// Does not rebuild FTS indexes - caller should call rebuild_fts() after batch operations.
    pub fn remove_file(&self, path: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        tx.execute("DELETE FROM files WHERE path = ?1", [path])?;
        tx.execute("DELETE FROM symbols WHERE file = ?1", [path])?;
        tx.execute("DELETE FROM texts WHERE file = ?1", [path])?;

        tx.commit()?;
        Ok(())
    }

    /// Upsert a single file and its symbols/texts.
    /// Removes old data for this path first, then inserts new data.
    /// Does not rebuild FTS indexes - caller should call rebuild_fts() after batch operations.
    pub fn upsert_file(
        &self,
        file: &FileEntry,
        symbols: &[SymbolEntry],
        texts: &[TextEntry],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Remove old data for this file
        tx.execute("DELETE FROM files WHERE path = ?1", [&file.path])?;
        tx.execute("DELETE FROM symbols WHERE file = ?1", [&file.path])?;
        tx.execute("DELETE FROM texts WHERE file = ?1", [&file.path])?;

        // Insert file
        tx.execute(
            "INSERT INTO files (path, lang, hash, lines) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![file.path, file.lang, file.hash, file.lines],
        )?;

        // Insert symbols
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols (file, name, kind, line_start, line_end, parent, sig, alias, visibility)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for s in symbols {
                stmt.execute(rusqlite::params![
                    s.file,
                    s.name,
                    s.kind,
                    s.line[0],
                    s.line[1],
                    s.parent,
                    s.sig,
                    s.alias,
                    s.visibility,
                ])?;
            }
        }

        // Insert texts
        {
            let mut stmt = tx.prepare(
                "INSERT INTO texts (file, kind, line_start, line_end, text, parent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for t in texts {
                stmt.execute(rusqlite::params![
                    t.file, t.kind, t.line[0], t.line[1], t.text, t.parent,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Rebuild all FTS5 indexes.
    /// Call this after batch upsert/remove operations.
    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            INSERT INTO files_fts(files_fts) VALUES('rebuild');
            INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');
            INSERT INTO texts_fts(texts_fts) VALUES('rebuild');
            ",
        )?;
        Ok(())
    }

    /// Export all data from DB back to vecs (for flushing to disk).
    pub fn export_all(&self) -> Result<(Vec<FileEntry>, Vec<SymbolEntry>, Vec<TextEntry>)> {
        let mut files = Vec::new();
        let mut symbols = Vec::new();
        let mut texts = Vec::new();

        // Export files
        {
            let mut stmt = self
                .conn
                .prepare("SELECT path, lang, hash, lines FROM files ORDER BY path")?;
            let rows = stmt.query_map([], |row| {
                Ok(FileEntry {
                    path: row.get(0)?,
                    lang: row.get(1)?,
                    hash: row.get(2)?,
                    lines: row.get(3)?,
                })
            })?;
            for row in rows {
                files.push(row?);
            }
        }

        // Export symbols
        {
            let mut stmt = self.conn.prepare(
                "SELECT file, name, kind, line_start, line_end, parent, sig, alias, visibility
                 FROM symbols
                 ORDER BY file, line_start",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(SymbolEntry {
                    file: row.get(0)?,
                    name: row.get(1)?,
                    kind: row.get(2)?,
                    line: [row.get(3)?, row.get(4)?],
                    parent: row.get(5)?,
                    sig: row.get(6)?,
                    alias: row.get(7)?,
                    visibility: row.get(8)?,
                })
            })?;
            for row in rows {
                symbols.push(row?);
            }
        }

        // Export texts
        {
            let mut stmt = self.conn.prepare(
                "SELECT file, kind, line_start, line_end, text, parent
                 FROM texts
                 ORDER BY file, line_start",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(TextEntry {
                    file: row.get(0)?,
                    kind: row.get(1)?,
                    line: [row.get(2)?, row.get(3)?],
                    text: row.get(4)?,
                    parent: row.get(5)?,
                })
            })?;
            for row in rows {
                texts.push(row?);
            }
        }

        Ok((files, symbols, texts))
    }
}

/// Quote a string for use in an FTS5 MATCH expression.
/// Wraps in double quotes and escapes any internal double quotes.
fn fts5_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
