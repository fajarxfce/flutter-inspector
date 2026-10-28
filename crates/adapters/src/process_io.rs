//! Cancellable, bounded line input shared by external process adapters.
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub struct LineReader<R> {
    reader: R,
    bytes: Vec<u8>,
    truncated: bool,
    limit: usize,
}
impl<R: AsyncBufRead + Unpin> LineReader<R> {
    pub fn new(reader: R, limit: usize) -> Self {
        Self {
            reader,
            bytes: Vec::new(),
            truncated: false,
            limit,
        }
    }
    // Buffer state survives select! cancellation while waiting for the next chunk.
    pub async fn next(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                return Ok((!self.bytes.is_empty() || self.truncated).then(|| self.finish()));
            }
            let end = chunk.iter().position(|b| *b == b'\n');
            let consumed = end.map_or(chunk.len(), |n| n + 1);
            let payload = end.unwrap_or(consumed);
            let keep = payload.min(self.limit.saturating_sub(self.bytes.len()));
            self.bytes.extend_from_slice(&chunk[..keep]);
            self.truncated |= keep < payload;
            self.reader.consume(consumed);
            if end.is_some() {
                return Ok(Some(self.finish()));
            }
        }
    }
    fn finish(&mut self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes)
            .trim_end_matches('\r')
            .to_owned();
        if self.truncated {
            text.push_str(" … [line truncated]");
        }
        self.bytes.clear();
        self.truncated = false;
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bounds_long_lines_without_losing_next_line() {
        let mut reader = LineReader::new(&b"123456789\nnext\r\nlast"[..], 5);
        assert_eq!(
            reader.next().await.unwrap().unwrap(),
            "12345 … [line truncated]"
        );
        assert_eq!(reader.next().await.unwrap().unwrap(), "next");
        assert_eq!(reader.next().await.unwrap().unwrap(), "last");
        assert!(reader.next().await.unwrap().is_none());
    }
}
