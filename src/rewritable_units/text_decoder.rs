use crate::base::SharedEncoding;
use crate::rewriter::RewritingError;
use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8};

pub(crate) struct TextDecoder {
    encoding: SharedEncoding,
    pending_text_streaming_decoder: Option<Decoder>,
    text_buffer: String,
}

impl TextDecoder {
    #[inline]
    #[must_use]
    pub fn new(encoding: SharedEncoding) -> Self {
        Self {
            encoding,
            pending_text_streaming_decoder: None,
            // TODO make adjustable
            text_buffer: String::from_utf8(vec![0u8; 1024]).unwrap(),
        }
    }

    #[inline]
    pub fn flush_pending(
        &mut self,
        output_handler: &mut dyn FnMut(&str, bool, &'static Encoding) -> Result<(), RewritingError>,
    ) -> Result<(), RewritingError> {
        if self.pending_text_streaming_decoder.is_some() {
            self.feed_text(&[], true, output_handler)?;
        }
        Ok(())
    }

    #[inline(never)]
    pub fn feed_text(
        &mut self,
        mut raw_input: &[u8],
        last_in_text_node: bool,
        output_handler: &mut dyn FnMut(&str, bool, &'static Encoding) -> Result<(), RewritingError>,
    ) -> Result<(), RewritingError> {
        let encoding = self.encoding.get();

        if let Some((utf8_text, rest)) = self.split_utf8_start(raw_input, encoding) {
            raw_input = rest;
            let really_last = last_in_text_node && rest.is_empty();

            (output_handler)(utf8_text, really_last, encoding)?;

            if really_last {
                debug_assert!(self.pending_text_streaming_decoder.is_none());
                return Ok(());
            }
        };

        let decoder = self
            .pending_text_streaming_decoder
            .get_or_insert_with(|| encoding.new_decoder_without_bom_handling());

        loop {
            let buffer = self.text_buffer.as_mut_str();
            let (status, read, written, ..) =
                decoder.decode_to_str(raw_input, buffer, last_in_text_node);

            let finished_decoding = status == CoderResult::InputEmpty;

            if written > 0 || last_in_text_node {
                // the last call to feed_text() may make multiple calls to output_handler,
                // but only one call to output_handler can be *the* last one.
                let really_last = last_in_text_node && finished_decoding;

                (output_handler)(
                    // this will always be in bounds, but unwrap_or_default optimizes better
                    buffer.get(..written).unwrap_or_default(),
                    really_last,
                    encoding,
                )?;
            }

            if finished_decoding {
                if last_in_text_node {
                    self.pending_text_streaming_decoder = None;
                }
                return Ok(());
            }
            raw_input = raw_input.get(read..).unwrap_or_default();
        }
    }

    /// Fast path for UTF-8 or ASCII prefix
    ///
    /// Returns UTF-8 text to emit + remaining bytes, or `None` if the fast path is not available
    #[inline]
    fn split_utf8_start<'i>(
        &self,
        raw_input: &'i [u8],
        encoding: &'static Encoding,
    ) -> Option<(&'i str, &'i [u8])> {
        // Can't use the fast path if the decoder may have buffered some bytes
        if self.pending_text_streaming_decoder.is_some() {
            return None;
        }

        let text_or_len = if encoding == UTF_8 {
            std::str::from_utf8(raw_input).map_err(|err| err.valid_up_to())
        } else {
            debug_assert!(encoding.is_ascii_compatible());
            Err(Encoding::ascii_valid_up_to(raw_input))
        };

        match text_or_len {
            Ok(utf8_text) => Some((utf8_text, &[][..])),
            Err(valid_up_to) => {
                // The slow path buffers 1KB, and even though this shouldn't matter,
                // it is an observable behavior, and it makes bugs worse for text handlers
                // that assume they'll get only a single chunk.
                if valid_up_to != raw_input.len() && valid_up_to < self.text_buffer.len() {
                    return None;
                }

                let (text, rest) = raw_input.split_at_checked(valid_up_to)?;
                Some((std::str::from_utf8(text).ok()?, rest))
            }
        }
    }
}
