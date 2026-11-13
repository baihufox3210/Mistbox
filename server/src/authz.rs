//! Authorization. Admin sessions may do everything; API keys are confined to
//! their scope node (a file, or a folder plus *all* current descendants at any
//! depth — evaluated live on every request, so moving an item out of the scope
//! immediately removes access) and to their list/read/write flags.
//!
//! Out-of-scope or hidden (sensitive) nodes yield 404 so a key cannot probe
//! for existence by guessing IDs; in-scope nodes lacking a flag yield 403.

use crate::{
    auth::Principal,
    error::{ApiError, ApiResult},
    nodes::{self, Node},
    AppState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perm {
    List,
    Read,
    Write,
}

impl Perm {
    fn label(self) -> &'static str {
        match self {
            Perm::List => "list",
            Perm::Read => "read",
            Perm::Write => "write",
        }
    }
}

/// Can the principal see this node at all?
pub async fn visible(state: &AppState, p: &Principal, node: &Node) -> ApiResult<bool> {
    match p {
        Principal::Admin { .. } => Ok(true),
        Principal::Key(k) => {
            if node.sensitive && !k.allow_sensitive {
                return Ok(false);
            }
            nodes::is_within(&state.db, &node.id, &k.scope_node_id).await
        }
    }
}

/// Require `perm` on `node`.
pub async fn require(state: &AppState, p: &Principal, node: &Node, perm: Perm) -> ApiResult<()> {
    let Principal::Key(k) = p else { return Ok(()) };
    if !visible(state, p, node).await? {
        return Err(ApiError::not_found());
    }
    let ok = match perm {
        Perm::List => k.can_list,
        Perm::Read => k.can_read,
        Perm::Write => k.can_write,
    };
    if !ok {
        return Err(ApiError::forbidden(format!("此 API Key 沒有 {} 權限", perm.label())));
    }
    Ok(())
}

/// Info requires list *or* read.
pub async fn require_info(state: &AppState, p: &Principal, node: &Node) -> ApiResult<()> {
    if let Principal::Key(k) = p {
        if !visible(state, p, node).await? {
            return Err(ApiError::not_found());
        }
        if !(k.can_list || k.can_read) {
            return Err(ApiError::forbidden("此 API Key 沒有 list 或 read 權限"));
        }
    }
    Ok(())
}

/// Rename / move-source / delete: key needs write, and the node must be
/// strictly inside the scope (a key can never alter its own scope root).
pub async fn require_modify(state: &AppState, p: &Principal, node: &Node) -> ApiResult<()> {
    require(state, p, node, Perm::Write).await?;
    if let Principal::Key(k) = p
        && node.id == k.scope_node_id {
            return Err(ApiError::forbidden("API Key 不能修改其授權範圍的根項目"));
        }
    Ok(())
}

/// Filter a listing down to what the principal can see.
pub async fn filter_visible(_state: &AppState, p: &Principal, list: Vec<Node>) -> ApiResult<Vec<Node>> {
    match p {
        Principal::Admin { .. } => Ok(list),
        // Children of a visible dir are inside the scope; only the sensitive flag matters.
        Principal::Key(k) => Ok(list.into_iter().filter(|n| !n.sensitive || k.allow_sensitive).collect()),
    }
}

/// Lookup + `require`, mapping missing and forbidden the same way.
pub async fn load(state: &AppState, p: &Principal, id: &str, perm: Perm) -> ApiResult<Node> {
    let node = nodes::get(&state.db, id).await?;
    require(state, p, &node, perm).await?;
    Ok(node)
}
