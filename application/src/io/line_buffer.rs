pub struct LineBuffer {
    buffer: Vec<u8>,
    line_start: usize,
}

/// Largest `n <= max` such that `buf[..n]` does not end in the middle of a UTF-8
/// character. A character is at most 4 bytes, so the search never looks further back
/// than that; on binary garbage (a long run of continuation bytes) it gives up and
/// returns `max` rather than stalling the buffer.
fn utf8_boundary(buf: &[u8], max: usize) -> usize {
    (max.saturating_sub(3)..=max)
        .rev()
        .find(|&n| buf[n] & 0xc0 != 0x80)
        .unwrap_or(max)
}

/// Rewrites every `" \r"` into `"\r\n"`, in place — Wings' `bytes.Replace(line, cr, crr)`
/// (`cr = " \r"`, `crr = "\r\n"`).
///
/// Games — Minecraft is the usual culprit — emit carriage returns wherever they think the
/// terminal wraps, and a program painting a progress bar emits nothing else until it is
/// done. Either way the frame has no `\n` to end it, so it sits in the buffer until
/// `MAX_LINE_LENGTH` forces a split and the panel receives a burst of repaints that have
/// already happened. Turning the return into a real line break gives every repaint its own
/// self-terminating frame, which ships immediately.
///
/// The pattern is `" \r"` rather than a bare `\r` on purpose: it leaves `\r\n` alone, and
/// it leaves a redraw that returns straight off a non-space character alone too, so those
/// still reach the terminal verbatim and repaint in place.
///
/// Both sides are two bytes, so nothing shifts and no allocation is needed.
fn rewrite_space_cr(buf: &mut [u8]) {
    for i in 1..buf.len() {
        if buf[i] == b'\r' && buf[i - 1] == b' ' {
            buf[i - 1] = b'\r';
            buf[i] = b'\n';
        }
    }
}

impl LineBuffer {
    const INITIAL_CAPACITY: usize = 10240; // 10 KiB
    // Matches Wings' `maxBufferSize` (64 KiB, wings/system/utils.go) — the point at which
    // it stops accumulating a single line. Was 5 KiB here, which forced a split roughly
    // 12× more often on any output that goes a long way without a `\n`.
    const MAX_LINE_LENGTH: usize = 65536; // 64 KiB
    const COMPACT_THRESHOLD: usize = 10240; // 10 KiB

    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(Self::INITIAL_CAPACITY),
            line_start: 0,
        }
    }

    pub fn extend(&mut self, data: &[u8]) {
        // One byte of overlap so a `" \r"` straddling two reads is still caught, but never
        // back past `line_start` — those bytes have already been handed out.
        let scan_from = self.line_start.max(self.buffer.len().saturating_sub(1));

        self.buffer.extend_from_slice(data);

        if let Some(tail) = self.buffer.get_mut(scan_from..) {
            rewrite_space_cr(tail);
        }
    }

    /// Next frame of container output — the terminating `\n` is included and nothing is
    /// trimmed. The bytes are those of the container's stream, with the one substitution
    /// `extend` makes (see [`rewrite_space_cr`]).
    ///
    /// Frames are fragments of a raw terminal byte stream, not display lines; consumers
    /// write them straight into a terminal emulator. That is what makes an in-place
    /// redraw work: a program painting a progress bar with `\r` emits far more than
    /// `MAX_LINE_LENGTH` bytes before its first `\n`, so it arrives as several frames,
    /// and only a frame that actually ends in `\n` starts a new row. Trimming used to
    /// eat the `\r` at exactly those seams, which is unrecoverable downstream — the
    /// panel had no way to tell a continuation from a new line.
    pub fn next_line(&mut self) -> Option<&[u8]> {
        let rest = self.buffer.get(self.line_start..)?;

        let len = match rest.iter().position(|&b| b == b'\n') {
            Some(pos) if pos < Self::MAX_LINE_LENGTH => pos + 1,
            // No newline within the chunk limit — force a split, but never mid-character:
            // the bytes are decoded with `from_utf8_lossy` downstream, and a straddled
            // character becomes one U+FFFD on *each* side of the seam, which is one cell
            // too many in a terminal grid and shifts the rest of the row.
            _ if rest.len() > Self::MAX_LINE_LENGTH => utf8_boundary(rest, Self::MAX_LINE_LENGTH),
            _ => return None,
        };

        let start = self.line_start;
        self.line_start += len;

        self.buffer.get(start..start + len)
    }

    pub fn compact(&mut self) {
        if self.line_start > Self::COMPACT_THRESHOLD && self.line_start > self.buffer.len() / 2 {
            self.buffer.drain(..self.line_start);
            self.line_start = 0;
        }
    }

    pub fn flush(&self) -> Option<&[u8]> {
        let rest = self.buffer.get(self.line_start..)?;
        (!rest.is_empty()).then_some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = LineBuffer::MAX_LINE_LENGTH;

    // LineBuffer
    //
    // Frames are verbatim slices of the container's byte stream. A frame ending in `\n`
    // is a complete line; one that does not is a forced mid-line split whose continuation
    // is the next frame. Nothing is trimmed — `\r` in particular has to survive, it is
    // the entire mechanism behind in-place redraws (progress bars, spinners).

    #[test]
    fn new_buffer_yields_nothing() {
        let mut lb = LineBuffer::new();
        assert_eq!(lb.next_line(), None);
        assert_eq!(lb.flush(), None);
    }

    #[test]
    fn single_line_keeps_its_newline() {
        let mut lb = LineBuffer::new();
        lb.extend(b"hello\n");
        assert_eq!(lb.next_line(), Some(&b"hello\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn multiple_lines_in_order() {
        let mut lb = LineBuffer::new();
        lb.extend(b"a\nb\nc\n");
        assert_eq!(lb.next_line(), Some(&b"a\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"b\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"c\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn pending_line_without_newline_returns_none() {
        let mut lb = LineBuffer::new();
        lb.extend(b"partial");
        assert_eq!(lb.next_line(), None);
        // still recoverable via flush
        assert_eq!(lb.flush(), Some(&b"partial"[..]));
    }

    #[test]
    fn partial_line_completed_by_later_extend() {
        let mut lb = LineBuffer::new();
        lb.extend(b"par");
        assert_eq!(lb.next_line(), None);
        lb.extend(b"tial\n");
        assert_eq!(lb.next_line(), Some(&b"partial\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn streaming_in_arbitrary_chunks() {
        let mut lb = LineBuffer::new();
        lb.extend(b"hel");
        assert_eq!(lb.next_line(), None);
        lb.extend(b"lo\nwor");
        assert_eq!(lb.next_line(), Some(&b"hello\n"[..]));
        assert_eq!(lb.next_line(), None);
        lb.extend(b"ld\n");
        assert_eq!(lb.next_line(), Some(&b"world\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn preserves_surrounding_whitespace() {
        let mut lb = LineBuffer::new();
        lb.extend(b"  hello  \n");
        assert_eq!(lb.next_line(), Some(&b"  hello  \n"[..]));
    }

    #[test]
    fn preserves_carriage_return_of_crlf() {
        let mut lb = LineBuffer::new();
        lb.extend(b"\thi\r\n");
        assert_eq!(lb.next_line(), Some(&b"\thi\r\n"[..]));
    }

    #[test]
    fn preserves_in_line_carriage_returns() {
        // One progress bar redrawing itself, then a real line break. The whole redraw
        // sequence must arrive intact or the terminal cannot replay it in place.
        let mut lb = LineBuffer::new();
        lb.extend(b"\r 10%\r 55%\r100%\ndone\n");
        assert_eq!(lb.next_line(), Some(&b"\r 10%\r 55%\r100%\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"done\n"[..]));
    }

    #[test]
    fn space_cr_becomes_a_line_break() {
        // Wings' substitution: a return that follows a space is a wrap the program guessed
        // at, not a redraw. It becomes a real break so the piece ships without waiting for
        // a `\n` that may never come.
        let mut lb = LineBuffer::new();
        lb.extend(b"loading \rloading. \rloading.. \rdone\n");
        assert_eq!(lb.next_line(), Some(&b"loading\r\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"loading.\r\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"loading..\r\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"done\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn space_cr_split_across_two_extends_is_still_caught() {
        let mut lb = LineBuffer::new();
        lb.extend(b"tick ");
        assert_eq!(lb.next_line(), None);
        lb.extend(b"\rtock\n");
        assert_eq!(lb.next_line(), Some(&b"tick\r\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"tock\n"[..]));
    }

    #[test]
    fn redraw_off_a_non_space_still_reaches_the_terminal_verbatim() {
        // The narrow pattern is the point: a progress bar returning straight off `%` is a
        // real in-place repaint and must not be turned into a new row.
        let mut lb = LineBuffer::new();
        lb.extend(b"[##..] 40%\r[###.] 60%\n");
        assert_eq!(lb.next_line(), Some(&b"[##..] 40%\r[###.] 60%\n"[..]));
    }

    #[test]
    fn crlf_is_left_alone() {
        let mut lb = LineBuffer::new();
        lb.extend(b"a\r\nb\r\n");
        assert_eq!(lb.next_line(), Some(&b"a\r\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"b\r\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn empty_line_is_a_bare_newline() {
        let mut lb = LineBuffer::new();
        lb.extend(b"a\n\nb\n");
        assert_eq!(lb.next_line(), Some(&b"a\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"\n"[..]));
        assert_eq!(lb.next_line(), Some(&b"b\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn whitespace_only_line_is_kept_as_is() {
        let mut lb = LineBuffer::new();
        lb.extend(b"   \n");
        assert_eq!(lb.next_line(), Some(&b"   \n"[..]));
    }

    #[test]
    fn flush_returns_remainder_verbatim() {
        let mut lb = LineBuffer::new();
        lb.extend(b"done\n  leftover  ");
        assert_eq!(lb.next_line(), Some(&b"done\n"[..]));
        assert_eq!(lb.flush(), Some(&b"  leftover  "[..]));
        assert_eq!(lb.flush(), Some(&b"  leftover  "[..]));
    }

    #[test]
    fn flush_is_none_when_fully_consumed() {
        let mut lb = LineBuffer::new();
        lb.extend(b"a\n");
        assert_eq!(lb.next_line(), Some(&b"a\n"[..]));
        assert_eq!(lb.flush(), None);
    }

    #[test]
    fn flush_of_whitespace_only_remainder_is_kept() {
        let mut lb = LineBuffer::new();
        lb.extend(b"x\n   ");
        assert_eq!(lb.next_line(), Some(&b"x\n"[..]));
        assert_eq!(lb.flush(), Some(&b"   "[..]));
    }

    #[test]
    fn line_exactly_max_splits_off_its_newline() {
        let mut lb = LineBuffer::new();
        let mut data = vec![b'x'; MAX];
        data.push(b'\n');
        lb.extend(&data);

        // the payload fills the chunk limit exactly, so it is emitted unterminated...
        assert_eq!(lb.next_line().map(<[_]>::len), Some(MAX));
        // ...and its newline follows as the next frame, which still ends the row.
        assert_eq!(lb.next_line(), Some(&b"\n"[..]));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn over_length_line_with_newline_is_split_into_chunks() {
        let mut lb = LineBuffer::new();
        let extra = 880;
        let mut data = vec![b'x'; MAX + extra];
        data.push(b'\n');
        lb.extend(&data);

        // first the forced max-length chunk, with no newline to mark it as a line end
        let first = lb.next_line().unwrap();
        assert_eq!(first.len(), MAX);
        assert_ne!(first.last(), Some(&b'\n'));
        // then the remainder including the newline
        assert_eq!(lb.next_line().map(<[_]>::len), Some(extra + 1));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn over_length_line_without_newline_is_force_emitted() {
        let mut lb = LineBuffer::new();
        let extra = 880;
        lb.extend(&vec![b'x'; MAX + extra]);

        // available > MAX with no newline forces a max-length chunk
        assert_eq!(lb.next_line().map(<[_]>::len), Some(MAX));
        // the leftover is below MAX and has no newline, so it waits
        assert_eq!(lb.next_line(), None);
        assert_eq!(lb.flush().map(<[_]>::len), Some(extra));
    }

    #[test]
    fn exactly_max_without_newline_waits_then_forces() {
        let mut lb = LineBuffer::new();
        lb.extend(&vec![b'x'; MAX]);
        // available == MAX is not strictly greater than MAX, so it waits
        assert_eq!(lb.next_line(), None);

        // one more byte tips it over and forces the chunk
        assert_eq!(lb.next_line(), None);
        lb.extend(b"x");
        assert_eq!(lb.next_line().map(<[_]>::len), Some(MAX));
        assert_eq!(lb.next_line(), None);
        assert_eq!(lb.flush().map(<[_]>::len), Some(1));
    }

    #[test]
    fn forced_chunk_remainder_completes_when_newline_arrives() {
        let mut lb = LineBuffer::new();
        let extra = 880;
        lb.extend(&vec![b'x'; MAX + extra]);
        assert_eq!(lb.next_line().map(<[_]>::len), Some(MAX));
        assert_eq!(lb.next_line(), None);

        lb.extend(b"\n");
        assert_eq!(lb.next_line().map(<[_]>::len), Some(extra + 1));
        assert_eq!(lb.next_line(), None);
    }

    #[test]
    fn forced_split_never_lands_mid_character() {
        // A multibyte character straddling the chunk limit must be pushed whole into the
        // next frame: split across frames it decodes as U+FFFD on both sides, which is
        // one cell too many and shifts every following column on that row.
        // 1 and 2 bytes of the character land inside the chunk; at 3 it ends exactly on
        // the limit and nothing straddles (covered by the next test).
        for lead_offset in 1..=2 {
            let mut lb = LineBuffer::new();
            let mut data = vec![b'x'; MAX - lead_offset];
            data.extend_from_slice("→".as_bytes()); // 3 bytes: e2 86 92
            data.extend_from_slice(&vec![b'y'; 64]);
            lb.extend(&data);

            let first = lb.next_line().unwrap().to_vec();
            assert_eq!(
                first.len(),
                MAX - lead_offset,
                "chunk should stop before the straddling character"
            );
            assert!(std::str::from_utf8(&first).is_ok());

            // and the character itself opens the continuation frame, intact
            let rest = lb.flush().unwrap();
            assert!(rest.starts_with("→".as_bytes()));
            assert!(std::str::from_utf8(rest).is_ok());
        }
    }

    #[test]
    fn forced_split_at_a_boundary_keeps_the_full_chunk() {
        // Character ends exactly at the limit — nothing straddles it, so no back-off.
        let mut lb = LineBuffer::new();
        let mut data = vec![b'x'; MAX - 3];
        data.extend_from_slice("→".as_bytes());
        data.extend_from_slice(&vec![b'y'; 64]);
        lb.extend(&data);

        assert_eq!(lb.next_line().map(<[_]>::len), Some(MAX));
    }

    #[test]
    fn forced_split_of_binary_garbage_still_makes_progress() {
        // A run of bare continuation bytes has no boundary to back off to; emitting the
        // full chunk beats stalling the buffer forever.
        let mut lb = LineBuffer::new();
        lb.extend(&vec![0x80u8; MAX + 64]);
        assert_eq!(lb.next_line().map(<[_]>::len), Some(MAX));
    }

    #[test]
    fn compact_noop_when_below_threshold() {
        let mut lb = LineBuffer::new();
        lb.extend(b"a\nbc");
        assert_eq!(lb.next_line(), Some(&b"a\n"[..]));
        let before = lb.line_start;

        lb.compact();
        // nothing drained, cursor untouched, data intact
        assert_eq!(lb.line_start, before);
        assert_eq!(lb.flush(), Some(&b"bc"[..]));
    }

    #[test]
    fn compact_drains_consumed_prefix_and_preserves_rest() {
        let mut lb = LineBuffer::new();

        let line_len = 100; // 99 payload bytes + '\n'
        let mut line = vec![b'x'; line_len - 1];
        line.push(b'\n');

        // enough lines that the consumed prefix clears COMPACT_THRESHOLD
        let total = LineBuffer::COMPACT_THRESHOLD / line_len + 30;
        let keep = 20;
        let consume = total - keep;

        for _ in 0..total {
            lb.extend(&line);
        }
        for _ in 0..consume {
            assert!(lb.next_line().is_some());
        }

        assert!(lb.line_start > LineBuffer::COMPACT_THRESHOLD);
        assert!(lb.line_start > lb.buffer.len() / 2);

        lb.compact();
        assert_eq!(lb.line_start, 0);
        assert_eq!(lb.buffer.len(), keep * line_len);

        let mut remaining = 0;
        while let Some(l) = lb.next_line() {
            assert_eq!(l.len(), line_len);
            remaining += 1;
        }
        assert_eq!(remaining, keep);
        assert_eq!(lb.flush(), None);
    }

    #[test]
    fn split_frames_reassemble_into_the_original_stream() {
        // The contract the panel relies on: concatenating every frame reproduces the
        // container's byte stream — with the one `" \r"` substitution applied — so writing
        // the frames into a terminal is indistinguishable from feeding it that stream.
        // Note this bar returns off a space, so it is the case Wings breaks into rows.
        let mut stream = Vec::new();
        stream.extend_from_slice(b"boot\n");
        for pct in 0..=100 {
            stream.extend_from_slice(format!("\rdownloading [{pct:>3}%] ██████ → ").as_bytes());
        }
        stream.extend_from_slice(b"\ndone\n");

        let mut lb = LineBuffer::new();
        let mut frames = Vec::new();
        for chunk in stream.chunks(777) {
            lb.extend(chunk);
            while let Some(frame) = lb.next_line() {
                frames.extend_from_slice(frame);
            }
            lb.compact();
        }
        if let Some(rest) = lb.flush() {
            frames.extend_from_slice(rest);
        }

        let mut expected = stream.clone();
        rewrite_space_cr(&mut expected);

        assert_eq!(frames, expected);
    }
}
