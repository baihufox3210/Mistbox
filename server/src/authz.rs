//! Authorization. Admin sessions may do everything; API keys and share links
//! (see `Principal::grant`) are confined to
//! their scope node (a file, or a folder plus *all* current descendants at any
//! depth — evaluated live on every request, so moving an item out of the scope
//! immediately removes access) and to their list/read/write flags.
//!
//! Out-of-scope or hidden (sensitive) nodes yield 404 so a key cannot probe
//! for existence by guessing IDs; in-scope nodes lacking a flag yield 403.

use crate::{
    AppState,
    auth::Principal,
    error::{ApiError, ApiResult},
    nodes::{self, Node},
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
    let Some(g) = p.grant() else { return Ok(true) };
    if node.sensitive && !g.allow_sensitive {
        return Ok(false);
    }
    // Keys and share links never see the recycle bin.
    Ok(nodes::is_within(&state.db, &node.id, g.scope).await?
        && !nodes::in_trash(&state.db, &node.id).await?)
}

/// Require `perm` on `node`.
pub async fn require(state: &AppState, p: &Principal, node: &Node, perm: Perm) -> ApiResult<()> {
    let Some(g) = p.grant() else { return Ok(()) };
    if !visible(state, p, node).await? {
        return Err(ApiError::not_found());
    }
    let ok = match perm {
        Perm::List => g.list,
        Perm::Read => g.read,
        Perm::Write => g.write,
    };
    if !ok {
        return Err(ApiError::forbidden(match (g.who, perm) {
            ("分享連結", Perm::Write) => "此分享連結只能檢視，不能修改".to_string(),
            (who, perm) => format!("此 {who} 沒有 {} 權限", perm.label()),
        }));
    }
    Ok(())
}

/// Info requires list *or* read.
pub async fn require_info(state: &AppState, p: &Principal, node: &Node) -> ApiResult<()> {
    if let Some(g) = p.grant() {
        if !visible(state, p, node).await? {
            return Err(ApiError::not_found());
        }
        if !(g.list || g.read) {
            return Err(ApiError::forbidden(format!(
                "此 {} 沒有 list 或 read 權限",
                g.who
            )));
        }
    }
    Ok(())
}

/// Rename / move-source / delete: needs write, and the node must be strictly
/// inside the scope (a key or share can never alter its own root item).
pub async fn require_modify(state: &AppState, p: &Principal, node: &Node) -> ApiResult<()> {
    require(state, p, node, Perm::Write).await?;
    if let Some(g) = p.grant()
        && node.id == g.scope
    {
        return Err(ApiError::forbidden(format!(
            "{}不能修改其分享／授權範圍的根項目",
            g.who
        )));
    }
    Ok(())
}

/// Filter a listing down to what the principal can see.
pub async fn filter_visible(
    _state: &AppState,
    p: &Principal,
    list: Vec<Node>,
) -> ApiResult<Vec<Node>> {
    match p.grant() {
        None => Ok(list),
        // Children of a visible dir are inside the scope; only the sensitive flag matters.
        Some(g) => Ok(list
            .into_iter()
            .filter(|n| !n.sensitive || g.allow_sensitive)
            .collect()),
    }
}

/// Lookup + `require`, mapping missing and forbidden the same way.
pub async fn load(state: &AppState, p: &Principal, id: &str, perm: Perm) -> ApiResult<Node> {
    let node = nodes::get(&state.db, id).await?;
    require(state, p, &node, perm).await?;
    Ok(node)
}
