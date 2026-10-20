//! `profile_utils` — port of upstream `src/services/user-profile/profile-utils.ts`.
//!
//! Only the two serialization helpers are ported: the per-item `centroid`/
//! `anchor` vectors are internal similarity state, and returning them to a
//! read-only consumer inflates the payload by hundreds of KB.
//!
//! Upstream's `safeArray`/`safeObject` are deliberately **not** ported here —
//! `ProfileItem::evidence` is `Option<Vec<String>>` and
//! `ProfileItem::evidence_or_empty()` already owns the "missing/invalid
//! evidence ⇒ empty" behavior, so a second coercion path would be dead code.

use serde_json::Value;

/// The sections whose items carry per-item embeddings.
const PROFILE_SECTIONS: [&str; 3] = ["preferences", "patterns", "workflows"];

/// Removes `centroid` and `anchor` from every item of every section.
///
/// Mutates `data` in place and hands the same borrow back (upstream's
/// `expect(result).toBe(data)`). Total over untrusted shapes: a non-object
/// root, a missing section, a non-array section and non-object array elements
/// are all no-ops.
pub fn strip_profile_embeddings(data: &mut Value) -> &mut Value {
    let Some(container) = data.as_object_mut() else {
        return data;
    };

    for section in PROFILE_SECTIONS {
        let Some(items) = container.get_mut(section).and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            let Some(fields) = item.as_object_mut() else {
                continue;
            };
            fields.remove("centroid");
            fields.remove("anchor");
        }
    }

    data
}

/// Deep-clones `data` and strips the embeddings from the clone, so the caller's
/// value keeps its vectors (upstream `toPublicProfileData`).
pub fn to_public_profile_data(data: &Value) -> Value {
    let mut cloned = data.clone();
    strip_profile_embeddings(&mut cloned);
    cloned
}
