use std::collections::BTreeMap;

/// One complete, client-owned document snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentSnapshot {
    uri: String,
    text: String,
    version: Option<i64>,
}

impl DocumentSnapshot {
    /// Construct a full document snapshot received from the protocol host.
    #[must_use]
    pub(crate) fn new(uri: String, text: String, version: Option<i64>) -> Self {
        Self { uri, text, version }
    }

    /// Return the document URI as supplied by the client.
    #[must_use]
    pub(crate) fn uri(&self) -> &str {
        &self.uri
    }

    /// Return the complete UTF-8 document text.
    #[must_use]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Return the client-assigned document version when supplied.
    #[must_use]
    pub(crate) const fn version(&self) -> Option<i64> {
        self.version
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ProtocolPosition {
    line: u32,
    character: u32,
}

impl ProtocolPosition {
    pub(crate) const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }

    pub(crate) const fn line(self) -> u32 {
        self.line
    }

    pub(crate) const fn character(self) -> u32 {
        self.character
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProtocolRange {
    start: ProtocolPosition,
    end: ProtocolPosition,
}

impl ProtocolRange {
    pub(crate) const fn new(start: ProtocolPosition, end: ProtocolPosition) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContentChange {
    range: Option<ProtocolRange>,
    range_length: Option<u32>,
    text: String,
}

impl ContentChange {
    pub(crate) fn new(
        range: Option<ProtocolRange>,
        range_length: Option<u32>,
        text: String,
    ) -> Self {
        Self {
            range,
            range_length,
            text,
        }
    }
}

/// The front-end document-store boundary.
///
/// The store retains complete UTF-8 client snapshots and applies incremental
/// LSP changes at the protocol boundary. Analysis crates only receive complete
/// snapshots, never protocol positions or edit objects.
#[derive(Debug, Default)]
pub(crate) struct DocumentStore {
    documents: BTreeMap<String, DocumentSnapshot>,
    revision: u64,
}

impl DocumentStore {
    /// Insert or replace a complete document snapshot.
    ///
    /// A replacement whose client version is not newer than the retained
    /// version is ignored.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, document: DocumentSnapshot) {
        let _ = self.replace(document);
    }

    pub(crate) fn replace(&mut self, document: DocumentSnapshot) -> bool {
        if self
            .documents
            .get(document.uri())
            .is_some_and(|current| !accepts_version(current.version, document.version))
        {
            return false;
        }
        if self.advance_revision().is_none() {
            return false;
        }
        let _ = self.documents.insert(document.uri.clone(), document);
        true
    }

    /// Remove one document by URI.
    pub(crate) fn remove(&mut self, uri: &str) -> Option<DocumentSnapshot> {
        if !self.documents.contains_key(uri) || self.advance_revision().is_none() {
            return None;
        }
        self.documents.remove(uri)
    }

    /// Look up a document by URI.
    #[must_use]
    pub(crate) fn get(&self, uri: &str) -> Option<&DocumentSnapshot> {
        self.documents.get(uri)
    }

    pub(crate) fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = &DocumentSnapshot> + DoubleEndedIterator {
        self.documents.values()
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    /// Return the number of open documents.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.documents.len()
    }

    /// Return whether the store contains no documents.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    pub(crate) fn apply_changes(
        &mut self,
        uri: &str,
        version: i64,
        changes: &[ContentChange],
    ) -> bool {
        if changes.is_empty() {
            return false;
        }
        let Some(document) = self.documents.get(uri) else {
            return false;
        };
        if !accepts_version(document.version, Some(version)) {
            return false;
        }
        let mut text = document.text.clone();
        if !changes.iter().all(|change| apply_change(&mut text, change)) {
            return false;
        }
        if self.advance_revision().is_none() {
            return false;
        }
        let Some(document) = self.documents.get_mut(uri) else {
            return false;
        };
        document.text = text;
        document.version = Some(version);
        true
    }

    fn advance_revision(&mut self) -> Option<u64> {
        let next = self.revision.checked_add(1)?;
        self.revision = next;
        Some(next)
    }
}

fn accepts_version(current: Option<i64>, incoming: Option<i64>) -> bool {
    match (current, incoming) {
        (Some(current), Some(incoming)) => incoming > current,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn apply_change(text: &mut String, change: &ContentChange) -> bool {
    let Some(range) = change.range else {
        if change
            .range_length
            .is_some_and(|length| utf16_length(text) != Some(length))
        {
            return false;
        }
        text.clear();
        text.push_str(&change.text);
        return true;
    };
    let Some(start) = byte_offset(text, range.start) else {
        return false;
    };
    let Some(end) = byte_offset(text, range.end) else {
        return false;
    };
    if start > end {
        return false;
    }
    if change
        .range_length
        .is_some_and(|length| utf16_length(&text[start..end]) != Some(length))
    {
        return false;
    }
    text.replace_range(start..end, &change.text);
    true
}

pub(crate) fn utf16_position(text: &str, offset: usize) -> Option<ProtocolPosition> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let line = u32::try_from(text[..offset].bytes().filter(|byte| *byte == b'\n').count()).ok()?;
    let start = text[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline.saturating_add(1));
    let character = utf16_length(&text[start..offset])?;
    Some(ProtocolPosition::new(line, character))
}

pub(crate) fn byte_offset(text: &str, position: ProtocolPosition) -> Option<usize> {
    let (start, end) = line_bounds(text, position.line)?;
    let mut units = 0_u32;
    for (offset, character) in text[start..end].char_indices() {
        if position.character == units {
            return Some(start.saturating_add(offset));
        }
        let next = units.checked_add(u32::try_from(character.len_utf16()).ok()?)?;
        if position.character < next {
            return None;
        }
        units = next;
    }
    (position.character == units).then_some(end)
}

fn line_bounds(text: &str, line: u32) -> Option<(usize, usize)> {
    let mut start = 0;
    for _ in 0..line {
        let newline = text[start..].find('\n')?;
        start = start.checked_add(newline)?.checked_add(1)?;
    }
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |newline| start.saturating_add(newline));
    Some((start, end))
}

fn utf16_length(text: &str) -> Option<u32> {
    text.chars().try_fold(0_u32, |length, character| {
        length.checked_add(u32::try_from(character.len_utf16()).ok()?)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ContentChange, DocumentSnapshot, DocumentStore, ProtocolPosition, ProtocolRange,
        byte_offset, utf16_position,
    };

    #[test]
    fn remove_of_a_missing_document_does_not_advance_the_revision() {
        // Guards the `!contains_key(uri) || advance_revision().is_none()`
        // guard's OR: a mutant weakening it to `&&` still evaluates
        // `advance_revision()` (Rust must, to know the `&&`'s result) even
        // though the URI is absent, silently bumping the revision counter
        // for a no-op removal.
        let mut store = DocumentStore::default();
        let revision_before = store.revision();
        assert_eq!(store.remove("untitled:missing"), None);
        assert_eq!(store.revision(), revision_before);
    }

    #[test]
    fn document_store_distinguishes_present_and_absent_documents() {
        let document = DocumentSnapshot::new(
            "file:///model.pure".to_owned(),
            "Class A{}".to_owned(),
            Some(3),
        );
        assert_eq!(document.uri(), "file:///model.pure");
        let mut documents = DocumentStore::default();
        assert!(documents.is_empty());
        assert_eq!(documents.len(), 0);
        assert_eq!(documents.get(document.uri()), None);
        documents.insert(document.clone());
        assert!(!documents.is_empty());
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.get(document.uri()), Some(&document));
        assert_eq!(documents.remove(document.uri()), Some(document.clone()));
        assert!(documents.is_empty());
        assert_eq!(documents.len(), 0);
        assert_eq!(documents.get(document.uri()), None);
        assert_eq!(documents.remove(document.uri()), None);
    }

    #[test]
    fn applies_unicode_utf16_changes_without_accepting_surrogate_halves() {
        let uri = "untitled:query";
        let mut documents = DocumentStore::default();
        documents.insert(DocumentSnapshot::new(
            uri.to_owned(),
            "/* 😀 */ [a,]".to_owned(),
            Some(1),
        ));

        let replacement = ContentChange::new(
            Some(ProtocolRange::new(
                ProtocolPosition::new(0, 12),
                ProtocolPosition::new(0, 13),
            )),
            Some(1),
            "b]".to_owned(),
        );
        assert!(documents.apply_changes(uri, 2, &[replacement]));
        assert_eq!(
            documents.get(uri).map(DocumentSnapshot::text),
            Some("/* 😀 */ [a,b]")
        );

        let invalid_surrogate_half = ContentChange::new(
            Some(ProtocolRange::new(
                ProtocolPosition::new(0, 4),
                ProtocolPosition::new(0, 5),
            )),
            Some(1),
            "x".to_owned(),
        );
        assert!(!documents.apply_changes(uri, 3, &[invalid_surrogate_half]));
        assert_eq!(
            documents.get(uri).and_then(DocumentSnapshot::version),
            Some(2)
        );
    }

    #[test]
    fn applies_multiple_changes_against_each_previous_snapshot() {
        let uri = "untitled:query";
        let mut documents = DocumentStore::default();
        documents.insert(DocumentSnapshot::new(
            uri.to_owned(),
            "abc".to_owned(),
            Some(1),
        ));

        let changes = [
            ContentChange::new(
                Some(ProtocolRange::new(
                    ProtocolPosition::new(0, 0),
                    ProtocolPosition::new(0, 1),
                )),
                Some(1),
                "x".to_owned(),
            ),
            ContentChange::new(
                Some(ProtocolRange::new(
                    ProtocolPosition::new(0, 1),
                    ProtocolPosition::new(0, 2),
                )),
                Some(1),
                "y".to_owned(),
            ),
        ];
        assert!(documents.apply_changes(uri, 2, &changes));
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("xyc"));
    }

    #[test]
    fn round_trips_crlf_positions_and_applies_astral_edits() {
        let text = "a😀\r\nb";
        for offset in [0, 1, 5, 6, 7, 8] {
            let position = utf16_position(text, offset).expect("character boundary position");
            assert_eq!(byte_offset(text, position), Some(offset));
        }

        let uri = "untitled:query";
        let mut documents = DocumentStore::default();
        documents.insert(DocumentSnapshot::new(
            uri.to_owned(),
            text.to_owned(),
            Some(1),
        ));
        let emoji = ContentChange::new(
            Some(ProtocolRange::new(
                ProtocolPosition::new(0, 1),
                ProtocolPosition::new(0, 3),
            )),
            Some(2),
            "x".to_owned(),
        );
        assert!(documents.apply_changes(uri, 2, &[emoji]));
        assert_eq!(
            documents.get(uri).map(DocumentSnapshot::text),
            Some("ax\r\nb")
        );
    }

    #[test]
    fn validates_utf16_range_lengths_and_keeps_rejected_changes_atomic() {
        let uri = "untitled:query";
        let mut documents = DocumentStore::default();
        assert!(documents.replace(DocumentSnapshot::new(
            uri.to_owned(),
            "a😀b".to_owned(),
            Some(1),
        )));
        assert_eq!(documents.revision(), 1);

        let emoji = ProtocolRange::new(ProtocolPosition::new(0, 1), ProtocolPosition::new(0, 3));
        assert!(documents.apply_changes(
            uri,
            2,
            &[ContentChange::new(Some(emoji), Some(2), "x".to_owned())],
        ));
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("axb"));
        assert_eq!(documents.revision(), 2);

        let changes = [
            ContentChange::new(
                Some(ProtocolRange::new(
                    ProtocolPosition::new(0, 0),
                    ProtocolPosition::new(0, 1),
                )),
                Some(1),
                "z".to_owned(),
            ),
            ContentChange::new(
                Some(ProtocolRange::new(
                    ProtocolPosition::new(0, 1),
                    ProtocolPosition::new(0, 2),
                )),
                Some(2),
                "y".to_owned(),
            ),
        ];
        assert!(!documents.apply_changes(uri, 3, &changes));
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("axb"));
        assert_eq!(
            documents.get(uri).and_then(DocumentSnapshot::version),
            Some(2)
        );
        assert_eq!(documents.revision(), 2);
    }

    #[test]
    fn rejects_stale_versions_without_advancing_the_store_revision() {
        let uri = "untitled:query";
        let mut documents = DocumentStore::default();
        assert!(documents.replace(DocumentSnapshot::new(
            uri.to_owned(),
            "[a]".to_owned(),
            Some(4),
        )));
        assert_eq!(documents.revision(), 1);

        assert!(!documents.replace(DocumentSnapshot::new(
            uri.to_owned(),
            "[stale]".to_owned(),
            Some(4),
        )));
        assert!(!documents.apply_changes(
            uri,
            3,
            &[ContentChange::new(None, None, "[stale]".to_owned())],
        ));
        assert!(!documents.apply_changes(uri, 5, &[]));
        assert_eq!(documents.revision(), 1);
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("[a]"));

        assert!(documents.apply_changes(
            uri,
            5,
            &[ContentChange::new(None, None, "[fresh]".to_owned())],
        ));
        assert_eq!(documents.revision(), 2);
        assert_eq!(
            documents.get(uri).and_then(DocumentSnapshot::version),
            Some(5)
        );
    }

    #[test]
    fn rejects_a_content_change_whose_range_end_precedes_its_start() {
        let uri = "untitled:reversed-range";
        let mut documents = DocumentStore::default();
        documents.insert(DocumentSnapshot::new(
            uri.to_owned(),
            "abcdef".to_owned(),
            Some(1),
        ));

        let reversed = ContentChange::new(
            Some(ProtocolRange::new(
                ProtocolPosition::new(0, 4),
                ProtocolPosition::new(0, 1),
            )),
            None,
            "x".to_owned(),
        );
        assert!(!documents.apply_changes(uri, 2, &[reversed]));
        assert_eq!(
            documents.get(uri).map(DocumentSnapshot::text),
            Some("abcdef"),
            "a reversed range must leave the document untouched"
        );
        assert_eq!(
            documents.get(uri).and_then(DocumentSnapshot::version),
            Some(1)
        );
        assert_eq!(documents.revision(), 1);
    }

    #[test]
    fn accepts_a_zero_width_range_as_a_pure_insertion() {
        // Guards `start > end`: a mutant weakening it to `>=` rejects the
        // equal case, which is exactly how an insertion at a cursor (an
        // empty selection) is expressed.
        let uri = "untitled:insertion";
        let mut documents = DocumentStore::default();
        documents.insert(DocumentSnapshot::new(
            uri.to_owned(),
            "ac".to_owned(),
            Some(1),
        ));

        let insertion = ContentChange::new(
            Some(ProtocolRange::new(
                ProtocolPosition::new(0, 1),
                ProtocolPosition::new(0, 1),
            )),
            None,
            "b".to_owned(),
        );
        assert!(documents.apply_changes(uri, 2, &[insertion]));
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("abc"));
    }

    #[test]
    fn full_document_replace_checks_the_range_length_against_the_current_text() {
        // Guards the whole-document-replace branch's
        // `utf16_length(text) != Some(length)` check. A mutant flipping it
        // to `==` inverts both directions: it would reject a replace whose
        // `range_length` correctly matches, and accept one whose
        // `range_length` is wrong.
        let uri = "untitled:query";
        let mut documents = DocumentStore::default();
        documents.insert(DocumentSnapshot::new(
            uri.to_owned(),
            "abc".to_owned(),
            Some(1),
        ));

        assert!(documents.apply_changes(
            uri,
            2,
            &[ContentChange::new(None, Some(3), "xyz".to_owned())],
        ));
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("xyz"));

        assert!(!documents.apply_changes(
            uri,
            3,
            &[ContentChange::new(
                None,
                Some(4),
                "should-not-apply".to_owned()
            )],
        ));
        assert_eq!(documents.get(uri).map(DocumentSnapshot::text), Some("xyz"));
    }

    #[test]
    fn utf16_position_refuses_an_offset_that_splits_a_multi_byte_character() {
        // `offset > text.len() || !text.is_char_boundary(offset)`: a mutant
        // weakening the OR to `&&` lets an in-bounds, non-boundary offset
        // (here byte 1 inside the two-byte 'é') fall through to
        // `text[..offset]`, which panics rather than returning `None`.
        assert_eq!(utf16_position("é", 1), None);
    }
}
