//! SQLite persistence: documents, chunks (+FTS5 BM25), meetings, transcripts, notes, cards.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocKind {
    Pdf,
    Word,
    Slides,
    Sheet,
    Markdown,
    Text,
    Html,
    Web,
    Meeting,
    Code,
    Image,
    Audio,
    Ebook,
}

impl DocKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DocKind::Pdf => "pdf",
            DocKind::Word => "word",
            DocKind::Slides => "slides",
            DocKind::Sheet => "sheet",
            DocKind::Markdown => "markdown",
            DocKind::Text => "text",
            DocKind::Html => "html",
            DocKind::Web => "web",
            DocKind::Meeting => "meeting",
            DocKind::Code => "code",
            DocKind::Image => "image",
            DocKind::Audio => "audio",
            DocKind::Ebook => "ebook",
        }
    }
    pub fn parse(s: &str) -> DocKind {
        match s {
            "pdf" => DocKind::Pdf,
            "word" => DocKind::Word,
            "slides" => DocKind::Slides,
            "sheet" => DocKind::Sheet,
            "markdown" => DocKind::Markdown,
            "html" => DocKind::Html,
            "web" => DocKind::Web,
            "meeting" => DocKind::Meeting,
            "code" => DocKind::Code,
            "image" => DocKind::Image,
            "audio" => DocKind::Audio,
            "ebook" => DocKind::Ebook,
            _ => DocKind::Text,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    pub id: i64,
    pub title: String,
    pub source: String,
    pub kind: DocKind,
    pub hash: String,
    pub bytes: u64,
    pub added_at: i64,
    pub chunk_count: i64,
    pub word_count: i64,
}

#[derive(Clone, Debug)]
pub struct ChunkRow {
    pub id: i64,
    pub doc_id: i64,
    pub ord: i64,
    pub section: String,
    pub page_start: i64,
    pub page_end: i64,
    pub text: String,
    pub token_count: i64,
}

#[derive(Clone, Debug)]
pub struct NewChunk {
    pub ord: i64,
    pub section: String,
    pub page_start: i64,
    pub page_end: i64,
    pub text: String,
    pub token_count: i64,
    pub embedding: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Meeting {
    pub id: i64,
    pub title: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_secs: i64,
    pub notes_raw: String,
    pub notes_enhanced: String,
    pub summary: String,
    pub segment_count: i64,
    pub card_count: i64,
}

#[derive(Clone, Debug)]
pub struct SegmentRow {
    pub id: i64,
    pub source: String,
    pub t0: f64,
    pub t1: f64,
    pub text: String,
    /// Who spoke (0, 1, 2...), when speakers have been told apart. `None` before that.
    pub speaker: Option<i64>,
}

/// A photo (or other file) attached to a meeting, with an optional model-written description.
#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: i64,
    pub kind: String,
    pub path: String,
    /// Seconds into the meeting when it was added; `None` when added afterwards.
    pub t: Option<f64>,
    pub caption: String,
}

/// A short note jotted during (or after) a meeting, with the model's cleaned-up version.
#[derive(Clone, Debug)]
pub struct Jot {
    pub id: i64,
    pub t: Option<f64>,
    pub kind: String,
    pub raw: String,
    pub polished: String,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub meetings: i64,
    pub seconds_transcribed: i64,
    pub words_transcribed: i64,
    pub documents: i64,
    pub chunks: i64,
    pub cards: i64,
    pub questions_answered: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA temp_store = MEMORY;
            PRAGMA cache_size = -8192;
            PRAGMA mmap_size = 67108864;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                source TEXT NOT NULL,
                kind TEXT NOT NULL,
                hash TEXT NOT NULL,
                bytes INTEGER NOT NULL DEFAULT 0,
                added_at INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL DEFAULT 0,
                word_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS documents_hash ON documents(hash);
            CREATE TABLE IF NOT EXISTS chunks (
                id INTEGER PRIMARY KEY,
                doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                ord INTEGER NOT NULL,
                section TEXT NOT NULL DEFAULT '',
                page_start INTEGER NOT NULL DEFAULT 0,
                page_end INTEGER NOT NULL DEFAULT 0,
                text TEXT NOT NULL,
                token_count INTEGER NOT NULL DEFAULT 0,
                embedding BLOB
            );
            CREATE INDEX IF NOT EXISTS chunks_doc ON chunks(doc_id, ord);
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                text, section, title,
                content='',
                tokenize='porter unicode61 remove_diacritics 2'
            );
            CREATE TABLE IF NOT EXISTS meetings (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                duration_secs INTEGER NOT NULL DEFAULT 0,
                notes_raw TEXT NOT NULL DEFAULT '',
                notes_enhanced TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS segments (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                source TEXT NOT NULL,
                t0 REAL NOT NULL,
                t1 REAL NOT NULL,
                text TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS segments_meeting ON segments(meeting_id, t0);
            CREATE TABLE IF NOT EXISTS meeting_sections (
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                from_line INTEGER NOT NULL,
                to_line INTEGER NOT NULL,
                t0 REAL NOT NULL,
                t1 REAL NOT NULL,
                text TEXT NOT NULL,
                PRIMARY KEY (meeting_id, from_line)
            );
            CREATE TABLE IF NOT EXISTS meeting_audio (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                offset_secs REAL NOT NULL DEFAULT 0,
                added_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS meeting_audio_meeting ON meeting_audio(meeting_id);
            CREATE TABLE IF NOT EXISTS document_files (
                doc_id INTEGER PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                mime TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ingest_queue (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                target TEXT NOT NULL,
                folder INTEGER,
                added_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS cards (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                question TEXT NOT NULL,
                answer TEXT NOT NULL DEFAULT '',
                citations TEXT NOT NULL DEFAULT '[]',
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                path TEXT NOT NULL,
                t REAL,
                caption TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS attachments_meeting ON attachments(meeting_id);
            CREATE TABLE IF NOT EXISTS jots (
                id INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                t REAL,
                kind TEXT NOT NULL DEFAULT 'note',
                raw TEXT NOT NULL,
                polished TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS jots_meeting ON jots(meeting_id);
            CREATE TABLE IF NOT EXISTS folders (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                color INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS counters (
                name TEXT PRIMARY KEY,
                value INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )?;
        // Folders arrived after the first release: add the columns to databases that predate them.
        for table in ["documents", "meetings"] {
            let has: bool = conn
                .prepare(&format!("PRAGMA table_info({table})"))?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .any(|c| c == "folder_id");
            if !has {
                conn.execute(&format!("ALTER TABLE {table} ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL"), [])?;
            }
        }
        // Speaker labels arrived later still.
        let has_speaker: bool = conn
            .prepare("PRAGMA table_info(segments)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|c| c == "speaker");
        if !has_speaker {
            conn.execute("ALTER TABLE segments ADD COLUMN speaker INTEGER", [])?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meeting_speakers (
                meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                idx INTEGER NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (meeting_id, idx)
             );
             CREATE INDEX IF NOT EXISTS documents_folder ON documents(folder_id);
             CREATE INDEX IF NOT EXISTS meetings_folder ON meetings(folder_id);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ---------- folders ----------

    pub fn create_folder(&self, name: &str, color: i64) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO folders(name,color,created_at) VALUES(?1,?2,?3)", params![name, color, now()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_folder(&self, id: i64, name: &str, color: i64) -> Result<()> {
        self.conn.lock().execute("UPDATE folders SET name=?1, color=?2 WHERE id=?3", params![name, color, id])?;
        Ok(())
    }

    /// Items in the folder become unfiled; nothing is deleted.
    pub fn delete_folder(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE documents SET folder_id=NULL WHERE folder_id=?1", params![id])?;
        conn.execute("UPDATE meetings SET folder_id=NULL WHERE folder_id=?1", params![id])?;
        conn.execute("DELETE FROM folders WHERE id=?1", params![id])?;
        Ok(())
    }

    /// (id, name, color, meetings, documents) in creation order.
    pub fn folders(&self) -> Result<Vec<(i64, String, i64, i64, i64)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT f.id, f.name, f.color,
                    (SELECT COUNT(*) FROM meetings m WHERE m.folder_id=f.id),
                    (SELECT COUNT(*) FROM documents d WHERE d.folder_id=f.id AND d.kind!='meeting')
             FROM folders f ORDER BY f.id",
        )?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_document_folder(&self, doc_id: i64, folder: Option<i64>) -> Result<()> {
        self.conn.lock().execute("UPDATE documents SET folder_id=?1 WHERE id=?2", params![folder, doc_id])?;
        Ok(())
    }

    /// A meeting and its indexed transcript always share a folder.
    pub fn set_meeting_folder(&self, meeting_id: i64, folder: Option<i64>) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE meetings SET folder_id=?1 WHERE id=?2", params![folder, meeting_id])?;
        conn.execute(
            "UPDATE documents SET folder_id=?1 WHERE source=?2",
            params![folder, format!("meeting://{meeting_id}")],
        )?;
        Ok(())
    }

    /// Copies the meeting's folder onto its indexed document (after the document is rebuilt).
    pub fn sync_meeting_doc_folder(&self, meeting_id: i64) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE documents SET folder_id=(SELECT folder_id FROM meetings WHERE id=?1) WHERE source=?2",
            params![meeting_id, format!("meeting://{meeting_id}")],
        )?;
        Ok(())
    }

    pub fn document_folders(&self) -> Result<std::collections::HashMap<i64, i64>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id, folder_id FROM documents WHERE folder_id IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    pub fn meeting_folders(&self) -> Result<std::collections::HashMap<i64, i64>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id, folder_id FROM meetings WHERE folder_id IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|m| m.filter_map(|r| r.ok()).collect())?;
        Ok(rows)
    }

    pub fn meeting_folder(&self, id: i64) -> Option<i64> {
        self.conn.lock().query_row("SELECT folder_id FROM meetings WHERE id=?1", params![id], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
    }

    /// Chunk ids that belong to documents in `folder`.
    pub fn chunk_ids_in_folder(&self, folder: i64) -> Result<std::collections::HashSet<i64>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT c.id FROM chunks c JOIN documents d ON d.id=c.doc_id WHERE d.folder_id=?1")?;
        let rows = st.query_map(params![folder], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    /// All chunks of a document in reading order: (ord, section, page_start, text).
    pub fn doc_chunks(&self, doc_id: i64) -> Result<Vec<(i64, String, i64, String)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT ord, section, page_start, text FROM chunks WHERE doc_id=?1 ORDER BY ord")?;
        let rows = st
            .query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- documents / chunks ----------

    pub fn find_document_by_hash(&self, hash: &str) -> Result<Option<Document>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT id,title,source,kind,hash,bytes,added_at,chunk_count,word_count FROM documents WHERE hash=?1",
        )?;
        let d = st.query_row(params![hash], row_to_doc).optional()?;
        Ok(d)
    }

    pub fn find_document_by_source(&self, source: &str) -> Result<Option<Document>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT id,title,source,kind,hash,bytes,added_at,chunk_count,word_count FROM documents WHERE source=?1",
        )?;
        Ok(st.query_row(params![source], row_to_doc).optional()?)
    }

    pub fn insert_document(
        &self,
        title: &str,
        source: &str,
        kind: DocKind,
        hash: &str,
        bytes: u64,
        word_count: i64,
    ) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO documents(title,source,kind,hash,bytes,added_at,chunk_count,word_count) VALUES(?1,?2,?3,?4,?5,?6,0,?7)",
            params![title, source, kind.as_str(), hash, bytes as i64, now(), word_count],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn insert_chunks(&self, doc_id: i64, title: &str, chunks: &[NewChunk]) -> Result<Vec<i64>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut ids = Vec::with_capacity(chunks.len());
        {
            let mut ins = tx.prepare(
                "INSERT INTO chunks(doc_id,ord,section,page_start,page_end,text,token_count,embedding) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            )?;
            let mut fts = tx.prepare("INSERT INTO chunks_fts(rowid,text,section,title) VALUES(?1,?2,?3,?4)")?;
            for c in chunks {
                let blob = f32s_to_bytes(&c.embedding);
                ins.execute(params![doc_id, c.ord, c.section, c.page_start, c.page_end, c.text, c.token_count, blob])?;
                let id = tx.last_insert_rowid();
                fts.execute(params![id, c.text, c.section, title])?;
                ids.push(id);
            }
        }
        tx.execute(
            "UPDATE documents SET chunk_count = chunk_count + ?1 WHERE id=?2",
            params![chunks.len() as i64, doc_id],
        )?;
        tx.commit()?;
        Ok(ids)
    }

    pub fn delete_document(&self, doc_id: i64) -> Result<Vec<i64>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let ids: Vec<i64> = {
            let mut st = tx.prepare("SELECT id FROM chunks WHERE doc_id=?1")?;
            let rows = st.query_map(params![doc_id], |r| r.get(0))?.collect::<std::result::Result<Vec<i64>, _>>()?;
            rows
        };
        {
            // Contentless FTS5 tables are updated through the 'delete' command.
            let mut del2 = tx.prepare("INSERT INTO chunks_fts(chunks_fts, rowid, text, section, title) VALUES('delete', ?1, ?2, ?3, ?4)")?;
            let mut sel = tx.prepare("SELECT c.text, c.section, d.title FROM chunks c JOIN documents d ON d.id=c.doc_id WHERE c.id=?1")?;
            for id in &ids {
                if let Ok((t, s, ti)) = sel.query_row(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
                    let _ = del2.execute(params![id, t, s, ti]);
                }
            }
        }
        tx.execute("DELETE FROM chunks WHERE doc_id=?1", params![doc_id])?;
        tx.execute("DELETE FROM documents WHERE id=?1", params![doc_id])?;
        tx.commit()?;
        Ok(ids)
    }

    pub fn list_documents(&self) -> Result<Vec<Document>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT id,title,source,kind,hash,bytes,added_at,chunk_count,word_count FROM documents ORDER BY added_at DESC",
        )?;
        let rows = st.query_map([], row_to_doc)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (chunk id, section, text) of a document's chunks, for embedding them again.
    pub fn doc_chunk_texts(&self, doc_id: i64) -> Result<Vec<(i64, String, String)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id, section, text FROM chunks WHERE doc_id=?1 ORDER BY ord")?;
        let rows = st
            .query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_chunk_embeddings(&self, items: &[(i64, Vec<f32>)]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut up = tx.prepare("UPDATE chunks SET embedding=?1 WHERE id=?2")?;
            for (id, v) in items {
                up.execute(params![f32s_to_bytes(v), id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// A named yes/no that outlives the process (which embedding model built the vectors, and so on).
    pub fn flag(&self, name: &str) -> bool {
        self.conn
            .lock()
            .query_row("SELECT value FROM counters WHERE name=?1", params![name], |r| r.get::<_, i64>(0))
            .map(|v| v > 0)
            .unwrap_or(false)
    }

    pub fn set_flag(&self, name: &str, on: bool) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO counters(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=?2",
            params![name, on as i64],
        )?;
        Ok(())
    }

    /// (chunk id, doc id, section, embedding) for every indexed chunk.
    pub fn all_embeddings(&self) -> Result<Vec<(i64, i64, String, Vec<f32>)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id, doc_id, section, embedding FROM chunks WHERE embedding IS NOT NULL")?;
        let rows = st
            .query_map([], |r| {
                let id: i64 = r.get(0)?;
                let doc: i64 = r.get(1)?;
                let section: String = r.get(2)?;
                let blob: Vec<u8> = r.get(3)?;
                Ok((id, doc, section, bytes_to_f32s(&blob)))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn chunks_by_ids(&self, ids: &[i64]) -> Result<Vec<(ChunkRow, Document)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT c.id,c.doc_id,c.ord,c.section,c.page_start,c.page_end,c.text,c.token_count,
                    d.id,d.title,d.source,d.kind,d.hash,d.bytes,d.added_at,d.chunk_count,d.word_count
             FROM chunks c JOIN documents d ON d.id=c.doc_id WHERE c.id IN ({placeholders})"
        );
        let mut st = conn.prepare(&sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                Ok((
                    ChunkRow {
                        id: r.get(0)?,
                        doc_id: r.get(1)?,
                        ord: r.get(2)?,
                        section: r.get(3)?,
                        page_start: r.get(4)?,
                        page_end: r.get(5)?,
                        text: r.get(6)?,
                        token_count: r.get(7)?,
                    },
                    Document {
                        id: r.get(8)?,
                        title: r.get(9)?,
                        source: r.get(10)?,
                        kind: DocKind::parse(&r.get::<_, String>(11)?),
                        hash: r.get(12)?,
                        bytes: r.get::<_, i64>(13)? as u64,
                        added_at: r.get(14)?,
                        chunk_count: r.get(15)?,
                        word_count: r.get(16)?,
                    },
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn neighbor_text(&self, doc_id: i64, ord: i64) -> Result<(Option<String>, Option<String>)> {
        let conn = self.conn.lock();
        let prev: Option<String> = conn
            .query_row("SELECT text FROM chunks WHERE doc_id=?1 AND ord=?2", params![doc_id, ord - 1], |r| r.get(0))
            .optional()?;
        let next: Option<String> = conn
            .query_row("SELECT text FROM chunks WHERE doc_id=?1 AND ord=?2", params![doc_id, ord + 1], |r| r.get(0))
            .optional()?;
        Ok((prev, next))
    }

    /// BM25 over the FTS5 index. Returns (chunk_id, score) with higher = better.
    pub fn bm25(&self, query: &str, limit: usize) -> Result<Vec<(i64, f32)>> {
        let fts_query = build_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT rowid, bm25(chunks_fts, 1.0, 0.4, 0.6) AS s FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY s LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![fts_query, limit as i64], |r| Ok((r.get::<_, i64>(0)?, -(r.get::<_, f64>(1)? as f32))))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---------- meetings ----------

    pub fn create_meeting(&self, title: &str) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO meetings(title,started_at) VALUES(?1,?2)", params![title, now()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_meeting_title(&self, id: i64, title: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE meetings SET title=?1 WHERE id=?2", params![title, id])?;
        Ok(())
    }

    pub fn update_meeting_notes(&self, id: i64, raw: &str, enhanced: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("UPDATE meetings SET notes_raw=?1, notes_enhanced=?2 WHERE id=?3", params![raw, enhanced, id])?;
        Ok(())
    }

    pub fn finish_meeting(&self, id: i64, duration_secs: i64) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE meetings SET ended_at=?1, duration_secs=?2 WHERE id=?3",
            params![now(), duration_secs, id],
        )?;
        Ok(())
    }

    /// Folds the write-ahead log into the main file, so copying `marginalia.sqlite3` alone is a complete backup.
    pub fn checkpoint(&self) -> Result<()> {
        let conn = self.conn.lock();
        let _: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        Ok(())
    }

    /// A stretch of kept audio: the file, and how many seconds into the meeting it starts.
    pub fn add_audio(&self, meeting_id: i64, path: &str, offset_secs: f64) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO meeting_audio(meeting_id,path,offset_secs,added_at) VALUES(?1,?2,?3,?4)", params![meeting_id, path, offset_secs, now()])?;
        Ok(conn.last_insert_rowid())
    }

    /// (path, offset seconds), in the order recorded.
    pub fn audios(&self, meeting_id: i64) -> Result<Vec<(String, f64)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT path, offset_secs FROM meeting_audio WHERE meeting_id=?1 ORDER BY offset_secs, id")?;
        let rows = st.query_map(params![meeting_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The original file of a document, kept so it can be opened again: (path, mime type).
    pub fn set_document_file(&self, doc_id: i64, path: &str, mime: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO document_files(doc_id,path,mime) VALUES(?1,?2,?3) ON CONFLICT(doc_id) DO UPDATE SET path=?2, mime=?3",
            params![doc_id, path, mime],
        )?;
        Ok(())
    }

    pub fn document_file(&self, doc_id: i64) -> Option<(String, String)> {
        self.conn
            .lock()
            .query_row("SELECT path,mime FROM document_files WHERE doc_id=?1", params![doc_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok()
    }

    /// Files and links waiting to be read, so a restart picks them up again.
    pub fn add_ingest(&self, kind: &str, target: &str, folder: Option<i64>) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO ingest_queue(kind,target,folder,added_at) VALUES(?1,?2,?3,?4)", params![kind, target, folder, now()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn remove_ingest(&self, id: i64) -> Result<()> {
        self.conn.lock().execute("DELETE FROM ingest_queue WHERE id=?1", params![id])?;
        Ok(())
    }

    /// (row id, kind, target, folder), oldest first.
    pub fn pending_ingests(&self) -> Result<Vec<(i64, String, String, Option<i64>)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id,kind,target,folder FROM ingest_queue ORDER BY id")?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Notes on a finished stretch of a long meeting, kept so a restart never redoes them.
    pub fn save_section(&self, meeting_id: i64, from: usize, to: usize, t0: f64, t1: f64, text: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO meeting_sections(meeting_id,from_line,to_line,t0,t1,text) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(meeting_id,from_line) DO UPDATE SET to_line=?3, t0=?4, t1=?5, text=?6",
            params![meeting_id, from as i64, to as i64, t0, t1, text],
        )?;
        Ok(())
    }

    /// (from line, to line, t0, t1, text), in order.
    pub fn sections(&self, meeting_id: i64) -> Result<Vec<(usize, usize, f64, f64, String)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT from_line,to_line,t0,t1,text FROM meeting_sections WHERE meeting_id=?1 ORDER BY from_line")?;
        let rows = st
            .query_map(params![meeting_id], |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// A meeting whose notes are completely written (summary, title, index). Recovery leaves it alone.
    pub fn mark_wrapped(&self, id: i64) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO counters(name,value) VALUES(?1,1) ON CONFLICT(name) DO UPDATE SET value=1",
            params![format!("wrapped_{id}")],
        )?;
        Ok(())
    }

    pub fn unmark_wrapped(&self, id: i64) -> Result<()> {
        self.conn.lock().execute("UPDATE counters SET value=0 WHERE name=?1", params![format!("wrapped_{id}")])?;
        Ok(())
    }

    /// Open a finished meeting again so more can be recorded into it.
    pub fn reopen_meeting(&self, id: i64) -> Result<()> {
        self.conn.lock().execute("UPDATE meetings SET ended_at=NULL WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn is_wrapped(&self, id: i64) -> bool {
        self.conn
            .lock()
            .query_row("SELECT value FROM counters WHERE name=?1", params![format!("wrapped_{id}")], |r| r.get::<_, i64>(0))
            .map(|v| v > 0)
            .unwrap_or(false)
    }

    pub fn set_meeting_summary(&self, id: i64, summary: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE meetings SET summary=?1 WHERE id=?2", params![summary, id])?;
        Ok(())
    }

    pub fn add_segment(&self, meeting_id: i64, source: &str, t0: f64, t1: f64, text: &str) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO segments(meeting_id,source,t0,t1,text) VALUES(?1,?2,?3,?4,?5)",
            params![meeting_id, source, t0, t1, text],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn segments(&self, meeting_id: i64) -> Result<Vec<SegmentRow>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id,source,t0,t1,text,speaker FROM segments WHERE meeting_id=?1 ORDER BY t0")?;
        let rows = st
            .query_map(params![meeting_id], |r| {
                Ok(SegmentRow { id: r.get(0)?, source: r.get(1)?, t0: r.get(2)?, t1: r.get(3)?, text: r.get(4)?, speaker: r.get(5)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Tell which speaker said each segment: (segment id, speaker). Replaces earlier labels.
    pub fn set_segment_speakers(&self, meeting_id: i64, labels: &[(i64, i64)]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("UPDATE segments SET speaker=NULL WHERE meeting_id=?1", params![meeting_id])?;
        {
            let mut up = tx.prepare("UPDATE segments SET speaker=?1 WHERE id=?2 AND meeting_id=?3")?;
            for (seg, spk) in labels {
                up.execute(params![spk, seg, meeting_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// (speaker, display name) for the speakers the person has named.
    pub fn speaker_names(&self, meeting_id: i64) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT idx,name FROM meeting_speakers WHERE meeting_id=?1 ORDER BY idx")?;
        let rows = st.query_map(params![meeting_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_speaker_name(&self, meeting_id: i64, idx: i64, name: &str) -> Result<()> {
        let conn = self.conn.lock();
        if name.trim().is_empty() {
            conn.execute("DELETE FROM meeting_speakers WHERE meeting_id=?1 AND idx=?2", params![meeting_id, idx])?;
        } else {
            conn.execute(
                "INSERT INTO meeting_speakers(meeting_id,idx,name) VALUES(?1,?2,?3) ON CONFLICT(meeting_id,idx) DO UPDATE SET name=?3",
                params![meeting_id, idx, name.trim()],
            )?;
        }
        Ok(())
    }

    /// Two speakers that were really one: everything `from` said becomes `to`.
    pub fn merge_speakers(&self, meeting_id: i64, from: i64, to: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE segments SET speaker=?1 WHERE meeting_id=?2 AND speaker=?3", params![to, meeting_id, from])?;
        conn.execute("DELETE FROM meeting_speakers WHERE meeting_id=?1 AND idx=?2", params![meeting_id, from])?;
        Ok(())
    }

    pub fn clear_speakers(&self, meeting_id: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE segments SET speaker=NULL WHERE meeting_id=?1", params![meeting_id])?;
        conn.execute("DELETE FROM meeting_speakers WHERE meeting_id=?1", params![meeting_id])?;
        Ok(())
    }

    pub fn add_card(&self, meeting_id: i64, question: &str, answer: &str, citations_json: &str) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO cards(meeting_id,question,answer,citations,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![meeting_id, question, answer, citations_json, now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Saved Live Assistance cards for a meeting, oldest first: (id, question, answer, citations json).
    pub fn cards(&self, meeting_id: i64) -> Result<Vec<(i64, String, String, String)>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id,question,answer,citations FROM cards WHERE meeting_id=?1 ORDER BY id")?;
        let rows = st
            .query_map(params![meeting_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_attachment(&self, meeting_id: i64, kind: &str, path: &str, t: Option<f64>) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO attachments(meeting_id,kind,path,t,caption,created_at) VALUES(?1,?2,?3,?4,'',?5)",
            params![meeting_id, kind, path, t, now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn set_attachment_caption(&self, id: i64, caption: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE attachments SET caption=?1 WHERE id=?2", params![caption, id])?;
        Ok(())
    }

    /// Attachments of a meeting in the order they happened (untimed ones last).
    pub fn attachments(&self, meeting_id: i64) -> Result<Vec<Attachment>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT id,kind,path,t,caption FROM attachments WHERE meeting_id=?1 ORDER BY t IS NULL, t, id",
        )?;
        let rows = st
            .query_map(params![meeting_id], |r| {
                Ok(Attachment { id: r.get(0)?, kind: r.get(1)?, path: r.get(2)?, t: r.get(3)?, caption: r.get(4)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Removes the row and returns the file path it pointed to.
    pub fn delete_attachment(&self, id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let path: Option<String> = conn.query_row("SELECT path FROM attachments WHERE id=?1", params![id], |r| r.get(0)).ok();
        conn.execute("DELETE FROM attachments WHERE id=?1", params![id])?;
        Ok(path)
    }

    pub fn add_jot(&self, meeting_id: i64, t: Option<f64>, kind: &str, raw: &str, polished: &str) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO jots(meeting_id,t,kind,raw,polished,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![meeting_id, t, kind, raw, polished, now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn set_jot_polished(&self, id: i64, polished: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE jots SET polished=?1 WHERE id=?2", params![polished, id])?;
        Ok(())
    }

    /// Replaces the wording and clears the old polish (it no longer matches).
    pub fn set_jot_raw(&self, id: i64, raw: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE jots SET raw=?1, polished='' WHERE id=?2", params![raw, id])?;
        Ok(())
    }

    pub fn delete_jot(&self, id: i64) -> Result<()> {
        self.conn.lock().execute("DELETE FROM jots WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Jots in the order they were taken (untimed ones last).
    pub fn jots(&self, meeting_id: i64) -> Result<Vec<Jot>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT id,t,kind,raw,polished FROM jots WHERE meeting_id=?1 ORDER BY t IS NULL, t, id")?;
        let rows = st
            .query_map(params![meeting_id], |r| Ok(Jot { id: r.get(0)?, t: r.get(1)?, kind: r.get(2)?, raw: r.get(3)?, polished: r.get(4)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_card_answer(&self, id: i64, answer: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE cards SET answer=?1 WHERE id=?2", params![answer, id])?;
        Ok(())
    }

    pub fn list_meetings(&self) -> Result<Vec<Meeting>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT m.id,m.title,m.started_at,m.ended_at,m.duration_secs,m.notes_raw,m.notes_enhanced,m.summary,
                    (SELECT COUNT(*) FROM segments s WHERE s.meeting_id=m.id),
                    (SELECT COUNT(*) FROM cards c WHERE c.meeting_id=m.id)
             FROM meetings m ORDER BY started_at DESC",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Meeting {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    started_at: r.get(2)?,
                    ended_at: r.get(3)?,
                    duration_secs: r.get(4)?,
                    notes_raw: r.get(5)?,
                    notes_enhanced: r.get(6)?,
                    summary: r.get(7)?,
                    segment_count: r.get(8)?,
                    card_count: r.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        self.conn.lock().execute("DELETE FROM meetings WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Drop meetings that captured nothing (no transcript, notes or cards), e.g. a recording
    /// stopped immediately or one left behind by a crash.
    pub fn prune_empty_meetings(&self, keep: Option<i64>) -> Result<usize> {
        let n = self.conn.lock().execute(
            "DELETE FROM meetings WHERE notes_raw='' AND notes_enhanced='' AND summary=''
               AND id IS NOT ?1
               AND NOT EXISTS (SELECT 1 FROM segments s WHERE s.meeting_id=meetings.id)
               AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.meeting_id=meetings.id)
               AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.meeting_id=meetings.id)
               AND NOT EXISTS (SELECT 1 FROM jots j WHERE j.meeting_id=meetings.id)",
            params![keep],
        )?;
        Ok(n)
    }

    pub fn bump_counter(&self, name: &str, by: i64) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO counters(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=value+?2",
            params![name, by],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<Stats> {
        let conn = self.conn.lock();
        let meetings: i64 = conn.query_row("SELECT COUNT(*) FROM meetings", [], |r| r.get(0))?;
        let seconds: f64 = conn
            .query_row("SELECT COALESCE(SUM(t1 - t0), 0) FROM segments", [], |r| r.get(0))
            .unwrap_or(0.0);
        let words: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(text) - LENGTH(REPLACE(text, ' ', '')) + 1), 0) FROM segments",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let documents: i64 = conn.query_row("SELECT COUNT(*) FROM documents WHERE kind != 'meeting'", [], |r| r.get(0))?;
        let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let cards: i64 = conn.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0))?;
        let questions_answered: i64 = conn
            .query_row("SELECT value FROM counters WHERE name='questions_answered'", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(Stats {
            meetings,
            seconds_transcribed: seconds as i64,
            words_transcribed: words,
            documents,
            chunks,
            cards,
            questions_answered,
        })
    }
}

fn row_to_doc(r: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
    Ok(Document {
        id: r.get(0)?,
        title: r.get(1)?,
        source: r.get(2)?,
        kind: DocKind::parse(&r.get::<_, String>(3)?),
        hash: r.get(4)?,
        bytes: r.get::<_, i64>(5)? as u64,
        added_at: r.get(6)?,
        chunk_count: r.get(7)?,
        word_count: r.get(8)?,
    })
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn f32s_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn bytes_to_f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Turn free text into a tolerant FTS5 query: quoted terms joined by OR, so
/// BM25 ranks by how many query terms a chunk covers.
pub fn build_fts_query(q: &str) -> String {
    let stop = [
        "the", "a", "an", "of", "to", "in", "on", "for", "and", "or", "is", "are", "was", "were", "be", "it", "this",
        "that", "what", "how", "do", "does", "did", "we", "you", "i", "can", "could", "should", "would", "with", "about",
        "our", "your", "their", "my", "me", "us", "at", "by", "from", "as", "if", "so", "there", "here", "when", "which",
        "who", "why", "will", "have", "has", "had", "not", "any", "some", "into", "than", "then", "them", "they", "he",
        "she", "his", "her", "its", "just", "also", "like", "know", "think", "okay", "ok", "yeah", "um", "uh",
    ];
    let mut terms: Vec<String> = Vec::new();
    for raw in q.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-') {
        let t = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if t.len() < 2 || stop.contains(&t.as_str()) || terms.contains(&t) {
            continue;
        }
        terms.push(t);
        if terms.len() >= 24 {
            break;
        }
    }
    terms.iter().map(|t| format!("\"{}\"", t.replace('"', ""))).collect::<Vec<_>>().join(" OR ")
}
