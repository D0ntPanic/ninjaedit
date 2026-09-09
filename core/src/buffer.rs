//! File buffers: in-memory containers for the contents of a loaded file.
//!
//! The buffer is optimized for fast editing of large text files. Contents are
//! stored as a sequence of chunks (each around [`CHUNK_TARGET`] bytes) so that
//! an insertion or deletion only rewrites one chunk rather than the whole
//! file. Line information is stored alongside the data: each chunk records the
//! offsets of the line breaks it contains, so global line queries only need to
//! walk the chunk headers, and an edit only re-scans the chunk it touched.
//!
//! Line endings: LF, CRLF, and lone CR are all recognized. The default ending
//! used when edits create new line breaks is the most common ending found in
//! the file at load time (ties prefer LF, then CRLF), or the platform native
//! ending for files that contain none. Existing bytes are never re-written on
//! load or save, so a file's original endings are preserved exactly.
//!
//! Saving is atomic: contents are written to a temporary file in the same
//! directory, synced, and renamed over the target, so a crash mid-save can
//! never leave a half-written file in place of the original.

use std::fs;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Target chunk size. Chunks are split when they grow past twice this.
const CHUNK_TARGET: usize = 16 * 1024;
/// Maximum size an in-place insertion may grow a chunk to before splitting.
const CHUNK_MAX: usize = CHUNK_TARGET * 2;

/// A line ending style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// The native line ending for the platform this was compiled for.
    pub fn native() -> LineEnding {
        if cfg!(windows) {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }
}

/// One chunk of buffer contents.
///
/// `breaks` holds, for every line terminator inside `data`, the offset just
/// past the terminator (the relative offset where the next line begins; this
/// may equal `data.len()`). A CRLF pair is a single terminator and is never
/// split across a chunk boundary (see `fix_crlf_boundaries`), so each chunk
/// can be scanned without looking at its neighbors.
///
/// Chunks are shared with [`BufferSnapshot`]s through an `Arc`, and are
/// copied on write: an edit clones the one chunk it touches if (and only
/// if) a snapshot still refers to it. See [`Chunk::make_mut`].
#[derive(Clone)]
struct Chunk(Arc<ChunkData>);

struct ChunkData {
    data: Vec<u8>,
    breaks: Vec<u32>,
}

impl std::ops::Deref for Chunk {
    type Target = ChunkData;

    fn deref(&self) -> &ChunkData {
        &self.0
    }
}

impl Chunk {
    fn new(data: Vec<u8>) -> Chunk {
        let mut chunk = ChunkData {
            data,
            breaks: Vec::new(),
        };
        chunk.rescan();
        Chunk(Arc::new(chunk))
    }

    /// Mutable access to the chunk, cloning its contents first if a
    /// snapshot shares them.
    fn make_mut(&mut self) -> &mut ChunkData {
        Arc::make_mut(&mut self.0)
    }
}

impl Clone for ChunkData {
    fn clone(&self) -> ChunkData {
        ChunkData {
            data: self.data.clone(),
            breaks: self.breaks.clone(),
        }
    }
}

impl ChunkData {
    fn len(&self) -> usize {
        self.data.len()
    }

    /// Rebuild the line break table from the chunk's contents.
    fn rescan(&mut self) {
        self.breaks.clear();
        let data = &self.data;
        let mut i = 0;
        while i < data.len() {
            match data[i] {
                b'\n' => {
                    self.breaks.push((i + 1) as u32);
                    i += 1;
                }
                b'\r' => {
                    if i + 1 < data.len() && data[i + 1] == b'\n' {
                        self.breaks.push((i + 2) as u32);
                        i += 2;
                    } else {
                        self.breaks.push((i + 1) as u32);
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
    }
}

/// Split raw bytes into chunks of roughly `CHUNK_TARGET` bytes, never
/// splitting a CRLF pair across a boundary.
fn make_chunks(bytes: &[u8]) -> Vec<Chunk> {
    let mut chunks = Vec::with_capacity(bytes.len() / CHUNK_TARGET + 1);
    let mut start = 0;
    while start < bytes.len() {
        let mut end = (start + CHUNK_TARGET).min(bytes.len());
        if end < bytes.len() && bytes[end - 1] == b'\r' && bytes[end] == b'\n' {
            end += 1;
        }
        chunks.push(Chunk::new(bytes[start..end].to_vec()));
        start = end;
    }
    chunks
}

/// Count each line ending style and return the most common one, falling back
/// to the platform native ending for files with no line breaks at all.
fn detect_eol(bytes: &[u8]) -> LineEnding {
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lf += 1;
                i += 1;
            }
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    crlf += 1;
                    i += 2;
                } else {
                    cr += 1;
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    if lf == 0 && crlf == 0 && cr == 0 {
        LineEnding::native()
    } else if lf >= crlf && lf >= cr {
        LineEnding::Lf
    } else if crlf >= cr {
        LineEnding::CrLf
    } else {
        LineEnding::Cr
    }
}

/// A loaded file's contents, structured for fast editing of large text files.
pub struct FileBuffer {
    chunks: Vec<Chunk>,
    eol: LineEnding,
    path: Option<PathBuf>,
    modified: bool,
}

impl FileBuffer {
    /// Create an empty buffer not associated with any file.
    pub fn new() -> FileBuffer {
        FileBuffer {
            chunks: Vec::new(),
            eol: LineEnding::native(),
            path: None,
            modified: false,
        }
    }

    /// Create a buffer from existing contents, not associated with any file.
    pub fn from_bytes(bytes: &[u8]) -> FileBuffer {
        FileBuffer {
            chunks: make_chunks(bytes),
            eol: detect_eol(bytes),
            path: None,
            modified: false,
        }
    }

    pub fn from_text(text: &str) -> FileBuffer {
        Self::from_bytes(text.as_bytes())
    }

    /// Load a file from disk.
    pub fn open(path: impl AsRef<Path>) -> io::Result<FileBuffer> {
        let path = path.as_ref();
        let bytes = fs::read(path)?;
        let mut buffer = Self::from_bytes(&bytes);
        buffer.path = Some(path.to_path_buf());
        Ok(buffer)
    }

    /// The file this buffer was loaded from or last saved to, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether the buffer has been edited since it was loaded or last saved.
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Override the modified flag. Used by the editor model, which tracks
    /// modification against its undo history so that undoing back to the
    /// saved state reports the buffer as unmodified again.
    pub(crate) fn set_modified(&mut self, modified: bool) {
        self.modified = modified;
    }

    /// The line ending used when edits create new line breaks.
    pub fn eol(&self) -> LineEnding {
        self.eol
    }

    pub fn set_eol(&mut self, eol: LineEnding) {
        self.eol = eol;
    }

    /// Total length in bytes.
    pub fn len(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|c| c.data.is_empty())
    }

    fn break_count(&self) -> usize {
        self.chunks.iter().map(|c| c.breaks.len()).sum()
    }

    /// Number of lines. An empty buffer has one (empty) line, and a buffer
    /// ending with a line terminator has a final empty line after it.
    pub fn line_count(&self) -> usize {
        self.break_count() + 1
    }

    /// A cheap, immutable copy of the current contents, for work on other
    /// threads. Taking one costs a clone of the chunk list (a few bytes per
    /// chunk); the chunk contents are shared until an edit touches them.
    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            chunks: self.chunks.clone(),
        }
    }

    /// Whether the buffer ends with a line terminator.
    fn ends_with_terminator(&self) -> bool {
        match self.chunks.last() {
            Some(c) => c.breaks.last() == Some(&(c.len() as u32)),
            None => false,
        }
    }

    /// Find the chunk containing `offset`, returning the chunk index and the
    /// offset relative to that chunk. `offset == len()` maps to the end of the
    /// last chunk.
    fn locate(&self, offset: usize) -> (usize, usize) {
        let mut remaining = offset;
        for (i, chunk) in self.chunks.iter().enumerate() {
            if remaining < chunk.len() {
                return (i, remaining);
            }
            remaining -= chunk.len();
        }
        if remaining == 0 && !self.chunks.is_empty() {
            let last = self.chunks.len() - 1;
            (last, self.chunks[last].len())
        } else if remaining == 0 {
            (0, 0)
        } else {
            panic!("offset {offset} out of bounds");
        }
    }

    /// Restore the invariant that a CRLF pair never spans a chunk boundary,
    /// by moving a leading LF into a preceding chunk that ends with CR. Only
    /// junctions can violate the invariant and only right after an edit, so a
    /// single cheap pass over the chunk headers suffices.
    fn fix_crlf_boundaries(&mut self) {
        let mut i = 1;
        while i < self.chunks.len() {
            let split_pair = self.chunks[i - 1].data.last() == Some(&b'\r')
                && self.chunks[i].data.first() == Some(&b'\n');
            if split_pair {
                self.chunks[i].make_mut().data.remove(0);
                let prev = self.chunks[i - 1].make_mut();
                prev.data.push(b'\n');
                prev.rescan();
                if self.chunks[i].data.is_empty() {
                    self.chunks.remove(i);
                } else {
                    self.chunks[i].make_mut().rescan();
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }

    /// Merge adjacent chunks that have shrunk, so heavy deletion doesn't leave
    /// the buffer fragmented into many tiny chunks.
    fn coalesce(&mut self) {
        self.chunks.retain(|c| !c.data.is_empty());
        let mut i = 0;
        while i + 1 < self.chunks.len() {
            if self.chunks[i].len() + self.chunks[i + 1].len() <= CHUNK_TARGET {
                let next = self.chunks.remove(i + 1);
                let chunk = self.chunks[i].make_mut();
                chunk.data.extend_from_slice(&next.data);
                chunk.rescan();
            } else {
                i += 1;
            }
        }
    }

    /// Insert text at a byte offset.
    pub fn insert(&mut self, offset: usize, text: &str) {
        self.insert_bytes(offset, text.as_bytes());
    }

    /// Insert raw bytes at a byte offset. Unlike [`insert`](Self::insert)
    /// this does not require valid UTF-8, so bytes copied out of the buffer
    /// can be put back exactly.
    pub fn insert_bytes(&mut self, offset: usize, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        assert!(offset <= self.len(), "insert offset {offset} out of bounds");
        if self.chunks.is_empty() {
            self.chunks = make_chunks(bytes);
        } else {
            let (index, rel) = self.locate(offset);
            let chunk = &mut self.chunks[index];
            if chunk.len() + bytes.len() <= CHUNK_MAX {
                let chunk = chunk.make_mut();
                chunk.data.splice(rel..rel, bytes.iter().copied());
                chunk.rescan();
            } else {
                let mut data = Vec::with_capacity(chunk.len() + bytes.len());
                data.extend_from_slice(&chunk.data[..rel]);
                data.extend_from_slice(bytes);
                data.extend_from_slice(&chunk.data[rel..]);
                let replacement = make_chunks(&data);
                self.chunks.splice(index..index + 1, replacement);
            }
        }
        self.fix_crlf_boundaries();
        self.modified = true;
    }

    /// Delete a byte range.
    pub fn delete(&mut self, range: Range<usize>) {
        assert!(
            range.start <= range.end && range.end <= self.len(),
            "delete range {range:?} out of bounds"
        );
        if range.is_empty() {
            return;
        }
        let (start_index, start_rel) = self.locate(range.start);
        let mut remaining = range.end - range.start;
        let mut index = start_index;
        let mut rel = start_rel;
        while remaining > 0 {
            let chunk = &mut self.chunks[index];
            let take = (chunk.len() - rel).min(remaining);
            if rel == 0 && take == chunk.len() {
                self.chunks.remove(index);
            } else {
                let chunk = chunk.make_mut();
                chunk.data.drain(rel..rel + take);
                chunk.rescan();
                index += 1;
            }
            remaining -= take;
            rel = 0;
        }
        self.fix_crlf_boundaries();
        self.coalesce();
        self.modified = true;
    }

    /// The byte offset at which a line starts. Accepts `0..line_count()`.
    pub fn offset_of_line(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let mut breaks_seen = 0;
        let mut base = 0;
        for chunk in &self.chunks {
            if breaks_seen + chunk.breaks.len() >= line {
                return base + chunk.breaks[line - breaks_seen - 1] as usize;
            }
            breaks_seen += chunk.breaks.len();
            base += chunk.len();
        }
        panic!("line {line} out of bounds");
    }

    /// The line containing a byte offset. An offset just past a terminator is
    /// on the following line; `offset == len()` maps to the last line.
    pub fn line_of_offset(&self, offset: usize) -> usize {
        assert!(offset <= self.len(), "offset {offset} out of bounds");
        let mut breaks_seen = 0;
        let mut base = 0;
        for chunk in &self.chunks {
            if offset < base + chunk.len() {
                let rel = (offset - base) as u32;
                let within = chunk.breaks.partition_point(|&b| b <= rel);
                return breaks_seen + within;
            }
            breaks_seen += chunk.breaks.len();
            base += chunk.len();
        }
        breaks_seen
    }

    /// The full byte range of a line, including its terminator if it has one.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        assert!(line < self.line_count(), "line {line} out of bounds");
        let start = self.offset_of_line(line);
        let end = if line < self.break_count() {
            self.offset_of_line(line + 1)
        } else {
            self.len()
        };
        start..end
    }

    /// The byte range of a line's content, excluding its terminator.
    pub fn line_content_range(&self, line: usize) -> Range<usize> {
        let range = self.line_range(line);
        if line >= self.break_count() {
            return range; // last line, no terminator
        }
        let terminator = if self.byte_at(range.end - 1) == b'\n'
            && range.end >= range.start + 2
            && self.byte_at(range.end - 2) == b'\r'
        {
            2
        } else {
            1
        };
        range.start..range.end - terminator
    }

    /// The byte at an offset. Panics if `offset >= len()`.
    pub fn byte_at(&self, offset: usize) -> u8 {
        let (index, rel) = self.locate(offset);
        self.chunks[index].data[rel]
    }

    /// Copy out a byte range.
    pub fn bytes_in_range(&self, range: Range<usize>) -> Vec<u8> {
        assert!(
            range.start <= range.end && range.end <= self.len(),
            "range {range:?} out of bounds"
        );
        let mut out = Vec::with_capacity(range.len());
        let mut base = 0;
        for chunk in &self.chunks {
            let chunk_start = base;
            let chunk_end = base + chunk.len();
            if chunk_end > range.start && chunk_start < range.end {
                let from = range.start.saturating_sub(chunk_start);
                let to = (range.end - chunk_start).min(chunk.len());
                out.extend_from_slice(&chunk.data[from..to]);
            }
            if chunk_end >= range.end {
                break;
            }
            base = chunk_end;
        }
        out
    }

    /// A line's content, excluding its terminator. Invalid UTF-8 is replaced.
    pub fn line_text(&self, line: usize) -> String {
        let bytes = self.bytes_in_range(self.line_content_range(line));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Replace a line's content, keeping its existing terminator.
    pub fn set_line(&mut self, line: usize, text: &str) {
        let range = self.line_content_range(line);
        self.delete(range.clone());
        self.insert(range.start, text);
    }

    /// Insert a new line so that `text` becomes line `line`, shifting later
    /// lines down. `line == line_count()` appends a line at the end (adding a
    /// terminator to the current last line if it doesn't have one).
    pub fn insert_line(&mut self, line: usize, text: &str) {
        let count = self.line_count();
        assert!(line <= count, "line {line} out of bounds");
        if line < count {
            let mut with_eol = String::with_capacity(text.len() + 2);
            with_eol.push_str(text);
            with_eol.push_str(self.eol.as_str());
            self.insert(self.offset_of_line(line), &with_eol);
        } else if !self.is_empty() && !self.ends_with_terminator() {
            let mut with_eol = String::with_capacity(text.len() + 2);
            with_eol.push_str(self.eol.as_str());
            with_eol.push_str(text);
            self.insert(self.len(), &with_eol);
        } else {
            self.insert(self.len(), text);
        }
    }

    /// Delete a line, including its terminator. Deleting the terminator-less
    /// last line also removes the preceding terminator so no empty line is
    /// left behind.
    pub fn delete_line(&mut self, line: usize) {
        let mut range = self.line_range(line);
        if range.end == self.len() && line > 0 && line >= self.break_count() {
            range.start = self.line_content_range(line - 1).end;
        }
        self.delete(range);
    }

    /// The entire contents as bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        for chunk in &self.chunks {
            out.extend_from_slice(&chunk.data);
        }
        out
    }

    /// The entire contents as text. Invalid UTF-8 is replaced.
    pub fn to_text(&self) -> String {
        String::from_utf8_lossy(&self.to_bytes()).into_owned()
    }

    /// Save to the buffer's associated file. Fails if the buffer has none.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "buffer has no associated file")
        })?;
        self.save_as(path)
    }

    /// Save to a file atomically: contents are written to a temporary file in
    /// the same directory, synced to disk, and renamed over the target, so an
    /// interrupted save never corrupts the existing file. The target's
    /// permissions are preserved.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        for chunk in &self.chunks {
            tmp.write_all(&chunk.data)?;
        }
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(tmp.path(), meta.permissions())?;
        }
        tmp.persist(path).map_err(|e| e.error)?;
        #[cfg(unix)]
        if let Ok(dir_handle) = fs::File::open(dir) {
            let _ = dir_handle.sync_all();
        }
        self.path = Some(path.to_path_buf());
        self.modified = false;
        Ok(())
    }
}

impl Default for FileBuffer {
    fn default() -> FileBuffer {
        FileBuffer::new()
    }
}

/// An immutable view of a [`FileBuffer`]'s contents at the moment
/// [`FileBuffer::snapshot`] was called. Snapshots are cheap to take and to
/// clone, and can be sent to other threads.
#[derive(Clone)]
pub struct BufferSnapshot {
    chunks: Vec<Chunk>,
}

impl BufferSnapshot {
    /// Total length in bytes.
    pub fn len(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|c| c.data.is_empty())
    }

    /// Number of lines, counted as [`FileBuffer::line_count`] does.
    pub fn line_count(&self) -> usize {
        self.chunks.iter().map(|c| c.breaks.len()).sum::<usize>() + 1
    }

    /// Iterate over line contents (without terminators) starting at `line`.
    /// Walking lines in order this way is much cheaper than looking each
    /// one up by number.
    pub fn lines_from(&self, line: usize) -> Lines<'_> {
        let mut remaining = line;
        let mut index = 0;
        let mut rel = 0;
        let mut brk = 0;
        while index < self.chunks.len() {
            let chunk = &self.chunks[index];
            if remaining <= chunk.breaks.len() {
                if remaining > 0 {
                    rel = chunk.breaks[remaining - 1] as usize;
                }
                brk = remaining;
                break;
            }
            remaining -= chunk.breaks.len();
            index += 1;
        }
        Lines {
            snapshot: self,
            index,
            rel,
            brk,
            done: line >= self.line_count(),
            scratch: Vec::new(),
        }
    }

    /// The entire contents as bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        for chunk in &self.chunks {
            out.extend_from_slice(&chunk.data);
        }
        out
    }
}

/// A forward walk over the lines of a [`BufferSnapshot`]; see
/// [`BufferSnapshot::lines_from`]. Not an `Iterator` because a line that
/// spans chunks is assembled in a scratch buffer that the next call reuses.
pub struct Lines<'a> {
    snapshot: &'a BufferSnapshot,
    /// The chunk the next line starts in.
    index: usize,
    /// The offset of the next line within that chunk.
    rel: usize,
    /// The index into that chunk's `breaks` of the next line's terminator.
    brk: usize,
    done: bool,
    scratch: Vec<u8>,
}

impl Lines<'_> {
    /// The content of the next line, or `None` after the last line.
    pub fn next_line(&mut self) -> Option<&[u8]> {
        if self.done {
            return None;
        }
        let chunks = &self.snapshot.chunks;
        // Fast path: the line ends inside the chunk it starts in.
        if let Some(chunk) = chunks.get(self.index)
            && let Some(&end) = chunk.breaks.get(self.brk)
        {
            let end = end as usize;
            let content = &chunk.data[self.rel..end - terminator_len(&chunk.data[..end])];
            self.rel = end;
            self.brk += 1;
            return Some(content);
        }
        // The line runs to the end of this chunk and on into the next ones
        // (or to the end of the buffer).
        self.scratch.clear();
        loop {
            let Some(chunk) = chunks.get(self.index) else {
                self.done = true;
                return Some(&self.scratch);
            };
            match chunk.breaks.get(self.brk) {
                Some(&end) => {
                    let end = end as usize;
                    self.scratch.extend_from_slice(
                        &chunk.data[self.rel..end - terminator_len(&chunk.data[..end])],
                    );
                    self.rel = end;
                    self.brk += 1;
                    return Some(&self.scratch);
                }
                None => {
                    self.scratch.extend_from_slice(&chunk.data[self.rel..]);
                    self.index += 1;
                    self.rel = 0;
                    self.brk = 0;
                }
            }
        }
    }
}

/// The length of the line terminator that `bytes` ends with (which it
/// must, and which never straddles a chunk boundary).
fn terminator_len(bytes: &[u8]) -> usize {
    if bytes.ends_with(b"\r\n") { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference implementation of line break offsets (offset just past each
    /// terminator) used to validate the chunked implementation.
    fn reference_breaks(bytes: &[u8]) -> Vec<usize> {
        let mut breaks = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    breaks.push(i + 1);
                    i += 1;
                }
                b'\r' => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                        breaks.push(i + 2);
                        i += 2;
                    } else {
                        breaks.push(i + 1);
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        breaks
    }

    fn check_consistency(buffer: &FileBuffer, expected: &str) {
        assert_eq!(buffer.to_text(), expected);
        let breaks = reference_breaks(expected.as_bytes());
        assert_eq!(buffer.line_count(), breaks.len() + 1);
        let mut starts = vec![0];
        starts.extend_from_slice(&breaks);
        for (line, &start) in starts.iter().enumerate() {
            assert_eq!(buffer.offset_of_line(line), start, "line {line} start");
        }
        for offset in [0, expected.len() / 2, expected.len()] {
            let line = buffer.line_of_offset(offset);
            let expected_line = starts.iter().filter(|&&s| s <= offset).count() - 1;
            assert_eq!(line, expected_line, "line of offset {offset}");
        }
    }

    #[test]
    fn snapshot_lines_and_copy_on_write() {
        // A file big enough to span several chunks, with lines that cross
        // chunk boundaries and every kind of terminator.
        let mut text = String::new();
        for i in 0..3000 {
            let eol = match i % 3 {
                0 => "\n",
                1 => "\r\n",
                _ => "\r",
            };
            text.push_str(&format!("line {i} {}{eol}", "x".repeat(i % 40)));
        }
        text.push_str("last line without terminator");
        let mut buffer = FileBuffer::from_text(&text);
        assert!(buffer.chunks.len() > 3);
        let snapshot = buffer.snapshot();
        assert_eq!(snapshot.line_count(), buffer.line_count());
        assert_eq!(snapshot.len(), buffer.len());

        for start in [0, 1, 1234, 2999, 3000] {
            let mut lines = snapshot.lines_from(start);
            for line in start..buffer.line_count() {
                let expected = buffer.line_text(line);
                let got = lines.next_line().expect("line present");
                assert_eq!(std::str::from_utf8(got).unwrap(), expected, "line {line}");
            }
            assert!(lines.next_line().is_none());
        }
        assert!(
            snapshot
                .lines_from(buffer.line_count())
                .next_line()
                .is_none()
        );

        // Editing the buffer leaves the snapshot untouched.
        buffer.insert(0, "/* ");
        buffer.delete(buffer.len() - 5..buffer.len());
        buffer.insert(buffer.len() / 2, &"y".repeat(CHUNK_MAX));
        assert_eq!(snapshot.to_bytes(), text.as_bytes());
        assert_ne!(buffer.to_text(), text);
        check_consistency(&buffer, &buffer.to_text());
    }

    #[test]
    fn empty_snapshot_has_one_empty_line() {
        let snapshot = FileBuffer::new().snapshot();
        assert_eq!(snapshot.line_count(), 1);
        let mut lines = snapshot.lines_from(0);
        assert_eq!(lines.next_line(), Some(&b""[..]));
        assert!(lines.next_line().is_none());
        let snapshot = FileBuffer::from_text("a\n").snapshot();
        let mut lines = snapshot.lines_from(0);
        assert_eq!(lines.next_line(), Some(&b"a"[..]));
        assert_eq!(lines.next_line(), Some(&b""[..]));
        assert!(lines.next_line().is_none());
    }

    #[test]
    fn eol_detection() {
        assert_eq!(FileBuffer::from_text("a\nb\nc\n").eol(), LineEnding::Lf);
        assert_eq!(
            FileBuffer::from_text("a\r\nb\r\nc\r\n").eol(),
            LineEnding::CrLf
        );
        assert_eq!(FileBuffer::from_text("a\rb\rc\r").eol(), LineEnding::Cr);
        assert_eq!(
            FileBuffer::from_text("a\r\nb\r\nc\n").eol(),
            LineEnding::CrLf,
            "most common ending wins"
        );
        assert_eq!(
            FileBuffer::from_text("no line breaks").eol(),
            LineEnding::native()
        );
        assert_eq!(FileBuffer::new().eol(), LineEnding::native());
    }

    #[test]
    fn empty_buffer() {
        let buffer = FileBuffer::new();
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.line_count(), 1);
        assert_eq!(buffer.line_text(0), "");
        assert_eq!(buffer.offset_of_line(0), 0);
        assert_eq!(buffer.line_of_offset(0), 0);
    }

    #[test]
    fn basic_insert_delete() {
        let mut buffer = FileBuffer::from_text("hello world");
        buffer.insert(5, ",");
        check_consistency(&buffer, "hello, world");
        buffer.delete(0..7);
        check_consistency(&buffer, "world");
        assert!(buffer.is_modified());
    }

    #[test]
    fn line_queries() {
        let buffer = FileBuffer::from_text("one\ntwo\r\nthree\rfour");
        assert_eq!(buffer.line_count(), 4);
        assert_eq!(buffer.line_text(0), "one");
        assert_eq!(buffer.line_text(1), "two");
        assert_eq!(buffer.line_text(2), "three");
        assert_eq!(buffer.line_text(3), "four");
        assert_eq!(buffer.line_of_offset(0), 0);
        assert_eq!(
            buffer.line_of_offset(3),
            0,
            "offset of the terminator itself"
        );
        assert_eq!(
            buffer.line_of_offset(4),
            1,
            "offset just past the terminator"
        );
        assert_eq!(buffer.line_of_offset(buffer.len()), 3);
    }

    #[test]
    fn trailing_terminator_yields_empty_last_line() {
        let buffer = FileBuffer::from_text("a\n");
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.line_text(0), "a");
        assert_eq!(buffer.line_text(1), "");
    }

    #[test]
    fn set_line_preserves_terminator() {
        let mut buffer = FileBuffer::from_text("a\r\nb\nc");
        buffer.set_line(0, "first");
        check_consistency(&buffer, "first\r\nb\nc");
        buffer.set_line(2, "last");
        check_consistency(&buffer, "first\r\nb\nlast");
    }

    #[test]
    fn insert_line() {
        let mut buffer = FileBuffer::from_text("a\nc");
        buffer.insert_line(1, "b");
        check_consistency(&buffer, "a\nb\nc");
        buffer.insert_line(3, "d");
        check_consistency(&buffer, "a\nb\nc\nd");
        buffer.insert_line(0, "start");
        check_consistency(&buffer, "start\na\nb\nc\nd");

        let mut crlf = FileBuffer::from_text("a\r\nb");
        crlf.insert_line(1, "mid");
        check_consistency(&crlf, "a\r\nmid\r\nb");
    }

    #[test]
    fn delete_line() {
        let mut buffer = FileBuffer::from_text("a\nb\nc");
        buffer.delete_line(1);
        check_consistency(&buffer, "a\nc");
        buffer.delete_line(1);
        check_consistency(&buffer, "a");
        buffer.delete_line(0);
        check_consistency(&buffer, "");

        let mut trailing = FileBuffer::from_text("a\n");
        trailing.delete_line(1);
        check_consistency(&trailing, "a");
    }

    #[test]
    fn multi_chunk_operations() {
        // Enough content to span several chunks.
        let mut expected = String::new();
        for i in 0..8000 {
            expected.push_str(&format!("line number {i}\n"));
        }
        let mut buffer = FileBuffer::from_text(&expected);
        assert!(buffer.chunks.len() > 3, "test should span multiple chunks");
        check_consistency(&buffer, &expected);

        assert_eq!(buffer.line_text(5000), "line number 5000");
        buffer.set_line(5000, "replaced");
        assert_eq!(buffer.line_text(5000), "replaced");
        assert_eq!(buffer.line_text(4999), "line number 4999");
        assert_eq!(buffer.line_text(5001), "line number 5001");

        // Delete a large range spanning many chunks.
        let start = buffer.offset_of_line(1000);
        let end = buffer.offset_of_line(7000);
        buffer.delete(start..end);
        assert_eq!(buffer.line_text(999), "line number 999");
        assert_eq!(buffer.line_text(1000), "line number 7000");
        assert_eq!(buffer.line_count(), 8000 - 6000 + 1);
    }

    #[test]
    fn crlf_across_chunk_boundary() {
        // A CRLF pair placed exactly at the chunk split point must still be
        // counted as a single terminator.
        let mut text = "x".repeat(CHUNK_TARGET - 1);
        text.push_str("\r\n");
        text.push_str(&"y".repeat(CHUNK_TARGET));
        let buffer = FileBuffer::from_text(&text);
        assert!(buffer.chunks.len() >= 2);
        assert_eq!(buffer.line_count(), 2);
        check_consistency(&buffer, &text);
    }

    #[test]
    fn insert_cr_before_chunk_leading_lf() {
        // Force a chunk boundary right before an LF, then insert a CR just
        // before it: the pair must be rejoined into a single terminator.
        let mut buffer = FileBuffer::from_text(&"x".repeat(CHUNK_TARGET * 2));
        let boundary = buffer.chunks[0].len();
        buffer.insert(boundary, "\n");
        let line_count = buffer.line_count();
        buffer.insert(boundary, "\r");
        assert_eq!(
            buffer.line_count(),
            line_count,
            "CR+LF must merge into one break"
        );
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            if bound == 0 {
                0
            } else {
                (self.next() % bound as u64) as usize
            }
        }
    }

    #[test]
    fn randomized_edits_match_reference() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let pieces = [
            "alpha",
            "b",
            "\n",
            "\r\n",
            "\r",
            "some longer text ",
            "\n\r\n\r",
        ];

        let mut expected = String::new();
        for _ in 0..20000 {
            expected.push_str(pieces[rng.below(pieces.len())]);
        }
        let mut buffer = FileBuffer::from_text(&expected);
        assert!(buffer.chunks.len() > 2);

        for step in 0..2000 {
            if rng.below(2) == 0 || expected.is_empty() {
                let offset = rng.below(expected.len() + 1);
                let piece = pieces[rng.below(pieces.len())];
                buffer.insert(offset, piece);
                expected.insert_str(offset, piece);
            } else {
                let start = rng.below(expected.len());
                let len = rng.below((expected.len() - start).min(4096) + 1);
                buffer.delete(start..start + len);
                expected.replace_range(start..start + len, "");
            }
            if step % 100 == 0 {
                check_consistency(&buffer, &expected);
            }
        }
        check_consistency(&buffer, &expected);
    }

    #[test]
    fn save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        let contents = "mixed\r\nendings\nhere\rlast";
        let mut buffer = FileBuffer::from_text(contents);
        buffer.save_as(&path).unwrap();
        assert!(!buffer.is_modified());
        assert_eq!(buffer.path(), Some(path.as_path()));

        let reloaded = FileBuffer::open(&path).unwrap();
        assert_eq!(
            reloaded.to_text(),
            contents,
            "endings must be preserved exactly"
        );
    }

    #[test]
    fn save_overwrites_atomically_and_preserves_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "original").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o754)).unwrap();
        }

        let mut buffer = FileBuffer::open(&path).unwrap();
        buffer.insert(0, "updated ");
        buffer.save().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "updated original");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o754);
        }
        // No stray temporary files left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
