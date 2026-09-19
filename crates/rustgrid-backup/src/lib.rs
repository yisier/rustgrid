//! RustGrid's own backup container, `.rgbak`.
//!
//! The format is deliberately simple, documented and versioned so it does not depend on any
//! vendor's private layout. A file is:
//!
//! ```text
//! [0..8)   magic           b"RGBACKUP"
//! [8..12)  format_version  u32 LE
//! [12..20) manifest_offset u64 LE   // absolute offset of the manifest
//!          ... chunk blocks ...      // raw zstd frames, addressed by the manifest
//!          [manifest_offset] u32 LE manifest_len, then `manifest_len` bytes of JSON
//! ```
//!
//! The JSON manifest lists every object (table/view/function/event) with its `CREATE` DDL,
//! column names, trigger DDL and one entry per data chunk. Each chunk entry carries the chunk's
//! absolute offset, compressed/uncompressed lengths and the uppercase-hex SHA-1 of its
//! compressed bytes. Data chunks hold newline-separated SQL value tuples such as
//! `(1, 'a', NULL)`, exactly like the driver dumps them.
//!
//! Because chunks are addressed by offset, restoring a subset of objects only decompresses the
//! selected objects' chunks — the rest are skipped without being read or inflated. Writes are
//! sequential (the manifest is appended last and its offset patched into the header), so no
//! second pass or full-file buffering is needed.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rustgrid_core::{BackupObjectKind, ObjectDump};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// File magic (`RGBACKUP`).
pub const MAGIC: &[u8; 8] = b"RGBACKUP";
/// The only format version this crate reads or writes.
pub const FORMAT_VERSION: u32 = 1;
/// The user-visible backup version shown in the details pane.
pub const BACKUP_VERSION: &str = "1.0.0";
/// File extension (without the dot) of a backup file.
pub const FILE_EXTENSION: &str = "rgbak";
/// Bytes in the fixed file header (`magic` + `format_version` + `manifest_offset`).
const HEADER_LEN: usize = 20;
/// Uncompressed bytes accumulated per data chunk before a new one starts.
pub const DATA_CHUNK_LIMIT: usize = 4 * 1024 * 1024;
/// zstd compression level. 3 is the library default and already much faster than gzip.
pub const COMPRESSION_LEVEL: i32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("backup I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid backup file: {0}")]
    Format(String),

    #[error("backup JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, BackupError>;

/// The root manifest of an `.rgbak` archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format_version: u32,
    /// The user-visible backup version, e.g. `1.0.0`.
    pub backup_version: String,
    /// The app version that wrote the file.
    pub app_version: String,
    /// The database the objects were read from.
    pub schema: String,
    pub created_unix: u64,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub objects: Vec<BackupObjectMeta>,
}

/// One backed-up object and the location of its data chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupObjectMeta {
    pub name: String,
    pub kind: BackupObjectKind,
    /// The object's `CREATE` statement.
    pub ddl: String,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub trigger_ddl: Vec<String>,
    #[serde(default)]
    pub row_count: usize,
    #[serde(default)]
    pub chunks: Vec<BackupChunkMeta>,
}

/// Address and integrity data for one zstd-compressed data chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupChunkMeta {
    /// Absolute file offset of the compressed bytes.
    pub offset: u64,
    pub uncompressed_len: u32,
    pub compressed_len: u32,
    /// Uppercase-hex SHA-1 of the compressed bytes.
    pub checksum: String,
}

/// A read object: metadata plus the concatenated (tuple-per-line) data text.
#[derive(Debug, Clone)]
pub struct RestoredObject {
    pub name: String,
    pub kind: BackupObjectKind,
    pub ddl: String,
    pub fields: Vec<String>,
    pub trigger_ddl: Vec<String>,
    /// Raw value tuples, one per line. Empty for views/functions/events.
    pub data: String,
}

/// A read archive.
#[derive(Debug, Clone)]
pub struct BackupArchive {
    pub manifest: BackupManifest,
    pub objects: Vec<RestoredObject>,
}

impl RestoredObject {
    /// Convert to the driver's dump type, splitting the stored data back into one tuple per line.
    pub fn to_object_dump(&self) -> ObjectDump {
        ObjectDump {
            name: self.name.clone(),
            kind: self.kind,
            ddl: self.ddl.clone(),
            fields: self.fields.clone(),
            trigger_ddl: self.trigger_ddl.clone(),
            rows: self
                .data
                .lines()
                .map(str::to_string)
                .filter(|line| !line.trim().is_empty())
                .collect(),
        }
    }
}

impl BackupArchive {
    /// Turn the archive into driver dumps for [`rustgrid_core::Connection::restore_object`].
    pub fn to_object_dumps(&self) -> Vec<ObjectDump> {
        self.objects
            .iter()
            .map(RestoredObject::to_object_dump)
            .collect()
    }
}

/// A random-access reader over one `.rgbak` file. Opening reads only the manifest; each object's
/// chunks are read and decompressed on demand, so a restore can process one object at a time
/// (bounded memory) and skip the rest entirely.
pub struct BackupReader {
    path: std::path::PathBuf,
    manifest: BackupManifest,
}

impl BackupReader {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            manifest: read_manifest(path)?,
        })
    }

    pub fn manifest(&self) -> &BackupManifest {
        &self.manifest
    }

    /// Read one object's data, or `None` when the archive has no such object.
    pub fn read_object(
        &self,
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<Option<RestoredObject>> {
        let Some(meta) = self
            .manifest
            .objects
            .iter()
            .find(|object| object.kind == kind && object.name == name)
        else {
            return Ok(None);
        };
        let mut file = File::open(&self.path)?;
        let data = read_object_data(&mut file, meta)?;
        Ok(Some(RestoredObject {
            name: meta.name.clone(),
            kind: meta.kind,
            ddl: meta.ddl.clone(),
            fields: meta.fields.clone(),
            trigger_ddl: meta.trigger_ddl.clone(),
            data,
        }))
    }
}

/// An incremental `.rgbak` writer.
///
/// Objects are added one at a time: [`BackupWriter::begin_object`] records the metadata,
/// [`BackupWriter::write_row`] streams value tuples (flushing a compressed chunk whenever the
/// buffer passes [`DATA_CHUNK_LIMIT`]), and [`BackupWriter::end_object`] seals the object.
/// [`BackupWriter::finish`] appends the manifest and patches the header. Memory stays bounded by
/// one chunk regardless of table size, and bytes reach disk as soon as the first rows arrive.
pub struct BackupWriter {
    file: File,
    offset: u64,
    schema: String,
    comment: String,
    created_unix: u64,
    objects: Vec<BackupObjectMeta>,
    current: Option<ObjectInProgress>,
    finished: bool,
}

struct ObjectInProgress {
    name: String,
    kind: BackupObjectKind,
    ddl: String,
    fields: Vec<String>,
    trigger_ddl: Vec<String>,
    row_count: usize,
    chunks: Vec<BackupChunkMeta>,
    buffer: String,
}

impl BackupWriter {
    /// Create the file and write its header placeholder.
    pub fn create(path: &Path, schema: &str, comment: &str) -> Result<Self> {
        let mut file = File::create(path)?;
        file.write_all(MAGIC)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())?;
        // Patched by `finish` once the manifest offset is known.
        file.write_all(&0u64.to_le_bytes())?;
        Ok(Self {
            file,
            offset: HEADER_LEN as u64,
            schema: schema.to_string(),
            comment: comment.to_string(),
            created_unix: unix_seconds(),
            objects: Vec::new(),
            current: None,
            finished: false,
        })
    }

    /// Start recording an object. Only its metadata is needed here; rows follow via `write_row`.
    pub fn begin_object(&mut self, object: &ObjectDump) -> Result<()> {
        if self.current.is_some() {
            return Err(BackupError::Format(
                "an object is already in progress".to_string(),
            ));
        }
        self.current = Some(ObjectInProgress {
            name: object.name.clone(),
            kind: object.kind,
            ddl: object.ddl.clone(),
            fields: object.fields.clone(),
            trigger_ddl: object.trigger_ddl.clone(),
            row_count: 0,
            chunks: Vec::new(),
            buffer: String::new(),
        });
        Ok(())
    }

    /// Append one rendered value tuple, flushing a chunk when the buffer is full.
    pub fn write_row(&mut self, tuple: &str) -> Result<()> {
        let should_flush = {
            let current = self.current.as_ref().ok_or_else(no_object_error)?;
            !current.buffer.is_empty() && current.buffer.len() + tuple.len() + 1 > DATA_CHUNK_LIMIT
        };
        if should_flush {
            self.flush_chunk()?;
        }
        let current = self.current.as_mut().ok_or_else(no_object_error)?;
        if !current.buffer.is_empty() {
            current.buffer.push('\n');
        }
        current.buffer.push_str(tuple);
        current.row_count += 1;
        Ok(())
    }

    /// Seal the current object, flushing its final chunk.
    pub fn end_object(&mut self) -> Result<()> {
        self.flush_chunk()?;
        let current = self.current.take().ok_or_else(no_object_error)?;
        self.objects.push(BackupObjectMeta {
            name: current.name,
            kind: current.kind,
            ddl: current.ddl,
            fields: current.fields,
            trigger_ddl: current.trigger_ddl,
            row_count: current.row_count,
            chunks: current.chunks,
        });
        Ok(())
    }

    /// Append the manifest and patch the header offset. Call once, after every object ended.
    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if self.current.is_some() {
            return Err(BackupError::Format(
                "an object is still in progress".to_string(),
            ));
        }
        let manifest = BackupManifest {
            format_version: FORMAT_VERSION,
            backup_version: BACKUP_VERSION.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            schema: std::mem::take(&mut self.schema),
            created_unix: self.created_unix,
            comment: std::mem::take(&mut self.comment),
            objects: std::mem::take(&mut self.objects),
        };
        let manifest_json = serde_json::to_vec(&manifest)?;
        let manifest_offset = self.offset;
        self.file
            .write_all(&clamp_u32(manifest_json.len()).to_le_bytes())?;
        self.file.write_all(&manifest_json)?;
        self.file.seek(SeekFrom::Start(12))?;
        self.file.write_all(&manifest_offset.to_le_bytes())?;
        self.file.flush()?;
        self.finished = true;
        Ok(())
    }

    fn flush_chunk(&mut self) -> Result<()> {
        let buffer = match self.current.as_mut() {
            Some(current) if !current.buffer.is_empty() => std::mem::take(&mut current.buffer),
            _ => return Ok(()),
        };
        let compressed = compress(buffer.as_bytes())?;
        let start = self.offset;
        self.file.write_all(&compressed)?;
        self.offset += compressed.len() as u64;
        let chunk = BackupChunkMeta {
            offset: start,
            uncompressed_len: clamp_u32(buffer.len()),
            compressed_len: clamp_u32(compressed.len()),
            checksum: sha1_hex(&compressed),
        };
        if let Some(current) = self.current.as_mut() {
            current.chunks.push(chunk);
        }
        Ok(())
    }
}

fn no_object_error() -> BackupError {
    BackupError::Format("no object in progress".to_string())
}

/// Write an `.rgbak` archive for one schema from fully-materialised objects. Streaming callers
/// should drive [`BackupWriter`] directly instead.
pub fn write_backup(
    path: &Path,
    schema: &str,
    objects: &[ObjectDump],
    comment: &str,
) -> Result<()> {
    let mut writer = BackupWriter::create(path, schema, comment)?;
    for object in objects {
        writer.begin_object(object)?;
        for row in &object.rows {
            writer.write_row(row)?;
        }
        writer.end_object()?;
    }
    writer.finish()
}

/// Read only the manifest of an archive (used to list backups without decompressing data).
pub fn read_manifest(path: &Path) -> Result<BackupManifest> {
    let mut file = File::open(path)?;
    read_manifest_from(&mut file)
}

/// Read every object of an archive, decompressing all chunks.
pub fn read_backup(path: &Path) -> Result<BackupArchive> {
    read_backup_selected(path, None)
}

/// Read the selected objects of an archive. `selected` names `(kind, name)` pairs; `None` reads
/// everything. Objects not selected are skipped without reading or decompressing their chunks.
pub fn read_backup_selected(
    path: &Path,
    selected: Option<&[(BackupObjectKind, String)]>,
) -> Result<BackupArchive> {
    let mut file = File::open(path)?;
    let manifest = read_manifest_from(&mut file)?;

    let mut objects = Vec::new();
    for meta in &manifest.objects {
        let wanted = selected.is_none_or(|list| {
            list.iter()
                .any(|(kind, name)| *kind == meta.kind && name == &meta.name)
        });
        if !wanted {
            continue;
        }

        let data = read_object_data(&mut file, meta)?;
        objects.push(RestoredObject {
            name: meta.name.clone(),
            kind: meta.kind,
            ddl: meta.ddl.clone(),
            fields: meta.fields.clone(),
            trigger_ddl: meta.trigger_ddl.clone(),
            data,
        });
    }

    Ok(BackupArchive { manifest, objects })
}

/// Read and decompress every chunk of one object, joining them with newlines.
fn read_object_data(file: &mut File, meta: &BackupObjectMeta) -> Result<String> {
    let mut data = String::new();
    for (index, chunk) in meta.chunks.iter().enumerate() {
        file.seek(SeekFrom::Start(chunk.offset))?;
        let mut compressed = vec![0u8; chunk.compressed_len as usize];
        file.read_exact(&mut compressed)?;
        if sha1_hex(&compressed) != chunk.checksum {
            return Err(BackupError::Format(format!(
                "checksum mismatch in {}.{}",
                meta.name, chunk.offset
            )));
        }
        let bytes = decompress(&compressed, chunk.uncompressed_len as usize)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| BackupError::Format(format!("{} data is not UTF-8", meta.name)))?;
        if index > 0 {
            data.push('\n');
        }
        data.push_str(&text);
    }
    Ok(data)
}

/// Render a standalone SQL script for one restored object: its DDL followed by batched
/// `INSERT` statements built from the stored tuples. Used by *Extract SQL*.
pub fn object_sql(object: &RestoredObject) -> String {
    let mut sql = String::new();
    let ddl = object.ddl.trim().trim_end_matches(';').trim_end();
    if !ddl.is_empty() {
        sql.push_str(ddl);
        sql.push_str(";\n");
    }

    let rows: Vec<&str> = object
        .data
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if !rows.is_empty() && !object.fields.is_empty() {
        let columns = object
            .fields
            .iter()
            .map(|field| format!("`{}`", field.replace('`', "``")))
            .collect::<Vec<_>>()
            .join(", ");
        let table = object.name.replace('`', "``");
        for batch in rows.chunks(INSERT_BATCH_ROWS) {
            sql.push_str(&format!(
                "INSERT INTO `{table}` ({columns}) VALUES\n{}\n;\n",
                batch.join(",\n")
            ));
        }
    }
    sql
}

/// Rows per `INSERT` statement in [`object_sql`].
pub const INSERT_BATCH_ROWS: usize = 500;

fn read_manifest_from(file: &mut File) -> Result<BackupManifest> {
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(BackupError::Format("not a RustGrid backup".to_string()));
    }
    let version = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if version != FORMAT_VERSION {
        return Err(BackupError::Format(format!(
            "unsupported backup format version {version}"
        )));
    }
    let manifest_offset = u64::from_le_bytes([
        header[12], header[13], header[14], header[15], header[16], header[17], header[18],
        header[19],
    ]);

    file.seek(SeekFrom::Start(manifest_offset))?;
    let mut length = [0u8; 4];
    file.read_exact(&mut length)?;
    let mut manifest_json = vec![0u8; u32::from_le_bytes(length) as usize];
    file.read_exact(&mut manifest_json)?;
    Ok(serde_json::from_slice(&manifest_json)?)
}

/// Split rendered tuples into chunks of roughly [`DATA_CHUNK_LIMIT`] uncompressed bytes, one
/// tuple per line. A single very large tuple still forms a chunk on its own.
#[cfg(test)]
fn data_chunks(rows: &[String]) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for row in rows {
        let extra = if current.is_empty() { 0 } else { 1 };
        if !current.is_empty() && current.len() + extra + row.len() > DATA_CHUNK_LIMIT {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(row);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn compress(data: &[u8]) -> Result<Vec<u8>> {
    zstd::bulk::compress(data, COMPRESSION_LEVEL).map_err(BackupError::Io)
}

fn decompress(data: &[u8], capacity: usize) -> Result<Vec<u8>> {
    zstd::bulk::decompress(data, capacity).map_err(BackupError::Io)
}

fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Clamp a length to `u32`; a single chunk never approaches 4 GiB in practice.
fn clamp_u32(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rustgrid-rgbak-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("test.{FILE_EXTENSION}"))
    }

    fn sample_objects() -> Vec<ObjectDump> {
        vec![
            ObjectDump {
                name: "users".to_string(),
                kind: BackupObjectKind::Table,
                ddl: "CREATE TABLE `users` (\n  `id` int NOT NULL,\n  `name` varchar(20)\n)"
                    .to_string(),
                fields: vec!["id".to_string(), "name".to_string()],
                trigger_ddl: vec![
                    "CREATE TRIGGER `t` BEFORE INSERT ON `users` FOR EACH ROW SET @x = 1"
                        .to_string(),
                ],
                rows: vec![
                    "(1, 'a')".to_string(),
                    "(2, NULL)".to_string(),
                    "(3, 'c\\\\d')".to_string(),
                ],
            },
            ObjectDump {
                name: "active_users".to_string(),
                kind: BackupObjectKind::View,
                ddl: "CREATE VIEW `active_users` AS SELECT * FROM `users`".to_string(),
                fields: Vec::new(),
                trigger_ddl: Vec::new(),
                rows: Vec::new(),
            },
        ]
    }

    #[test]
    fn round_trips_an_archive() {
        let path = temp_path();
        write_backup(&path, "demo", &sample_objects(), "hello").unwrap();

        let manifest = read_manifest(&path).unwrap();
        assert_eq!(manifest.format_version, FORMAT_VERSION);
        assert_eq!(manifest.schema, "demo");
        assert_eq!(manifest.comment, "hello");
        assert_eq!(manifest.objects.len(), 2);
        assert_eq!(manifest.objects[0].kind, BackupObjectKind::Table);
        assert_eq!(manifest.objects[0].row_count, 3);
        assert_eq!(manifest.objects[1].kind, BackupObjectKind::View);

        let archive = read_backup(&path).unwrap();
        assert_eq!(archive.objects.len(), 2);
        assert_eq!(archive.objects[0].name, "users");
        assert_eq!(archive.objects[0].kind, BackupObjectKind::Table);
        assert_eq!(
            archive.objects[0].data.lines().collect::<Vec<_>>(),
            vec!["(1, 'a')", "(2, NULL)", "(3, 'c\\\\d')"]
        );
        assert_eq!(archive.objects[0].trigger_ddl.len(), 1);
        assert_eq!(archive.objects[1].kind, BackupObjectKind::View);
        assert!(archive.objects[1].data.is_empty());

        let dumps = archive.to_object_dumps();
        assert_eq!(dumps[0].rows.len(), 3);

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn streaming_writer_round_trips() {
        let path = temp_path();
        {
            let mut writer = BackupWriter::create(&path, "demo", "streamed").unwrap();
            for object in sample_objects() {
                writer.begin_object(&object).unwrap();
                for row in &object.rows {
                    writer.write_row(row).unwrap();
                }
                writer.end_object().unwrap();
            }
            writer.finish().unwrap();
        }

        let archive = read_backup(&path).unwrap();
        assert_eq!(archive.manifest.comment, "streamed");
        assert_eq!(archive.manifest.objects[0].row_count, 3);
        assert_eq!(archive.objects.len(), 2);
        assert_eq!(archive.objects[0].data.lines().count(), 3);

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn reads_a_selected_subset() {
        let path = temp_path();
        write_backup(&path, "demo", &sample_objects(), "").unwrap();

        let selected = vec![(BackupObjectKind::View, "active_users".to_string())];
        let archive = read_backup_selected(&path, Some(&selected)).unwrap();
        assert_eq!(archive.objects.len(), 1);
        assert_eq!(archive.objects[0].name, "active_users");

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn reader_reads_one_object_on_demand() {
        let path = temp_path();
        write_backup(&path, "demo", &sample_objects(), "").unwrap();

        let reader = BackupReader::open(&path).unwrap();
        assert_eq!(reader.manifest().objects.len(), 2);

        let users = reader
            .read_object(BackupObjectKind::Table, "users")
            .unwrap()
            .expect("users");
        assert_eq!(users.data.lines().count(), 3);
        assert_eq!(users.to_object_dump().rows.len(), 3);

        assert!(
            reader
                .read_object(BackupObjectKind::Table, "missing")
                .unwrap()
                .is_none()
        );

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn chunks_large_data() {
        // 8 MiB of tuples must split into at least two chunks.
        let row = format!("({}, '{}')", 1, "x".repeat(1024));
        let rows: Vec<String> = std::iter::repeat_n(row, 8 * 1024).collect();
        let chunks = data_chunks(&rows);
        assert!(chunks.len() >= 2, "got {} chunks", chunks.len());
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.lines().count())
                .sum::<usize>(),
            rows.len()
        );
    }

    #[test]
    fn checksums_are_sha1_hex() {
        let path = temp_path();
        write_backup(&path, "demo", &sample_objects(), "").unwrap();
        let manifest = read_manifest(&path).unwrap();
        for object in &manifest.objects {
            for chunk in &object.chunks {
                assert_eq!(chunk.checksum.len(), 40);
                assert!(chunk.checksum.chars().all(|c| c.is_ascii_hexdigit()));
            }
        }
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn rejects_a_foreign_file() {
        let path = temp_path();
        std::fs::write(&path, b"not a backup at all........").unwrap();
        assert!(read_manifest(&path).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
