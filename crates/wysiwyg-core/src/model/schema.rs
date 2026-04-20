use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::attrs::{AttrValue, Attrs};
use super::mark::MarkTypeId;
use super::node::NodeTypeId;

/// Specification for an attribute on a node or mark type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttrSpec {
    /// Optional default value.  If `None`, the attribute is required.
    pub default: Option<AttrValue>,
}

/// Specification for a node type — provided when constructing a `Schema`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    /// Content expression string, e.g. "block+", "inline*", "text*".
    /// `None` means no content (leaf/atom).
    pub content: Option<String>,
    /// Space-separated group names this node belongs to, e.g. "block" or "inline".
    pub group: Option<String>,
    /// Whether this is an inline node.
    pub inline: bool,
    /// Whether this is an atom (indivisible from editing perspective).
    pub atom: bool,
    /// Which marks are allowed inside ("_" = all, "" = none).
    pub marks: Option<String>,
    /// Default attributes and their specs.
    pub attrs: HashMap<String, AttrSpec>,
}

impl NodeSpec {
    pub fn is_leaf(&self) -> bool {
        self.content.is_none()
    }

    pub fn is_inline(&self) -> bool {
        self.inline
    }

    pub fn is_block(&self) -> bool {
        !self.inline
    }
}

/// Specification for a mark type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkSpec {
    /// Space-separated group names.
    pub group: Option<String>,
    /// If true, the mark extends to adjacent typed text.
    pub inclusive: bool,
    /// Mark types this mark excludes (space-separated type names; "_" = all).
    pub excludes: Option<String>,
    pub attrs: HashMap<String, AttrSpec>,
}

/// A resolved node type within the schema.
#[derive(Debug)]
pub struct NodeType {
    pub id: NodeTypeId,
    pub name: Arc<str>,
    pub spec: NodeSpec,
}

impl NodeType {
    /// Build default attributes from the spec (filling in defaults).
    pub fn default_attrs(&self) -> Attrs {
        let map: std::collections::BTreeMap<Arc<str>, AttrValue> = self
            .spec
            .attrs
            .iter()
            .filter_map(|(k, spec)| spec.default.clone().map(|v| (Arc::from(k.as_str()), v)))
            .collect();
        Attrs::from(map)
    }
}

/// A resolved mark type within the schema.
#[derive(Debug)]
pub struct MarkType {
    pub id: MarkTypeId,
    pub name: Arc<str>,
    pub spec: MarkSpec,
}

impl MarkType {
    pub fn default_attrs(&self) -> Attrs {
        let map: std::collections::BTreeMap<Arc<str>, AttrValue> = self
            .spec
            .attrs
            .iter()
            .filter_map(|(k, spec)| spec.default.clone().map(|v| (Arc::from(k.as_str()), v)))
            .collect();
        Attrs::from(map)
    }
}

/// The schema: a registry of node types and mark types.
///
/// Constructed once and then shared via `Arc<Schema>` across all `EditorState`
/// instances.  The schema is immutable after construction.
///
/// # Phase 1 note
/// Content expression parsing and validation are deferred to Phase 2.
/// In this phase, schema just stores type registrations.
#[derive(Debug)]
pub struct Schema {
    pub nodes: Vec<Arc<NodeType>>,
    pub marks: Vec<Arc<MarkType>>,
    node_by_name: HashMap<Arc<str>, NodeTypeId>,
    mark_by_name: HashMap<Arc<str>, MarkTypeId>,
    /// The top-level node type (typically "doc").
    pub top_node: NodeTypeId,
}

impl Schema {
    /// Build a schema from ordered node and mark spec lists.
    ///
    /// The first entry in `nodes` becomes the `top_node`.
    pub fn new(
        node_specs: Vec<(impl Into<Arc<str>>, NodeSpec)>,
        mark_specs: Vec<(impl Into<Arc<str>>, MarkSpec)>,
    ) -> Self {
        let nodes: Vec<Arc<NodeType>> = node_specs
            .into_iter()
            .enumerate()
            .map(|(i, (name, spec))| {
                Arc::new(NodeType {
                    id: NodeTypeId(i as u16),
                    name: name.into(),
                    spec,
                })
            })
            .collect();

        let marks: Vec<Arc<MarkType>> = mark_specs
            .into_iter()
            .enumerate()
            .map(|(i, (name, spec))| {
                Arc::new(MarkType {
                    id: MarkTypeId(i as u16),
                    name: name.into(),
                    spec,
                })
            })
            .collect();

        let node_by_name: HashMap<Arc<str>, NodeTypeId> =
            nodes.iter().map(|nt| (nt.name.clone(), nt.id)).collect();

        let mark_by_name: HashMap<Arc<str>, MarkTypeId> =
            marks.iter().map(|mt| (mt.name.clone(), mt.id)).collect();

        let top_node = nodes
            .first()
            .expect("Schema must have at least one node type")
            .id;

        Schema {
            nodes,
            marks,
            node_by_name,
            mark_by_name,
            top_node,
        }
    }

    pub fn node_type(&self, id: NodeTypeId) -> &Arc<NodeType> {
        &self.nodes[id.0 as usize]
    }

    pub fn mark_type(&self, id: MarkTypeId) -> &Arc<MarkType> {
        &self.marks[id.0 as usize]
    }

    pub fn node_type_by_name(&self, name: &str) -> Option<&Arc<NodeType>> {
        self.node_by_name
            .get(name)
            .map(|id| &self.nodes[id.0 as usize])
    }

    pub fn mark_type_by_name(&self, name: &str) -> Option<&Arc<MarkType>> {
        self.mark_by_name
            .get(name)
            .map(|id| &self.marks[id.0 as usize])
    }
}

/// Construct the standard "basic" schema with built-in node and mark types.
///
/// Node types (in order, so `doc` is the top node):
///   doc, paragraph, heading, code_block, blockquote,
///   bullet_list, ordered_list, list_item, hard_break, text
///
/// Mark types:
///   bold, italic, code, link
pub fn basic_schema() -> Arc<Schema> {
    use std::collections::HashMap;

    let nodes: Vec<(String, NodeSpec)> = vec![
        (
            "doc".into(),
            NodeSpec {
                content: Some("block+".into()),
                group: None,
                inline: false,
                atom: false,
                marks: None,
                attrs: HashMap::new(),
            },
        ),
        (
            "paragraph".into(),
            NodeSpec {
                content: Some("inline*".into()),
                group: Some("block".into()),
                inline: false,
                atom: false,
                marks: Some("_".into()),
                attrs: HashMap::new(),
            },
        ),
        (
            "heading".into(),
            NodeSpec {
                content: Some("inline*".into()),
                group: Some("block".into()),
                inline: false,
                atom: false,
                marks: Some("_".into()),
                attrs: {
                    let mut m = HashMap::new();
                    m.insert(
                        "level".into(),
                        AttrSpec {
                            default: Some(AttrValue::Int(1)),
                        },
                    );
                    m
                },
            },
        ),
        (
            "code_block".into(),
            NodeSpec {
                content: Some("text*".into()),
                group: Some("block".into()),
                inline: false,
                atom: false,
                marks: Some("".into()),
                attrs: {
                    let mut m = HashMap::new();
                    m.insert(
                        "language".into(),
                        AttrSpec {
                            default: Some(AttrValue::String("".into())),
                        },
                    );
                    m
                },
            },
        ),
        (
            "blockquote".into(),
            NodeSpec {
                content: Some("block+".into()),
                group: Some("block".into()),
                inline: false,
                atom: false,
                marks: None,
                attrs: HashMap::new(),
            },
        ),
        (
            "bullet_list".into(),
            NodeSpec {
                content: Some("list_item+".into()),
                group: Some("block".into()),
                inline: false,
                atom: false,
                marks: None,
                attrs: HashMap::new(),
            },
        ),
        (
            "ordered_list".into(),
            NodeSpec {
                content: Some("list_item+".into()),
                group: Some("block".into()),
                inline: false,
                atom: false,
                marks: None,
                attrs: {
                    let mut m = HashMap::new();
                    m.insert(
                        "start".into(),
                        AttrSpec {
                            default: Some(AttrValue::Int(1)),
                        },
                    );
                    m
                },
            },
        ),
        (
            "list_item".into(),
            NodeSpec {
                content: Some("block+".into()),
                group: None,
                inline: false,
                atom: false,
                marks: None,
                attrs: HashMap::new(),
            },
        ),
        (
            "hard_break".into(),
            NodeSpec {
                content: None,
                group: Some("inline".into()),
                inline: true,
                atom: true,
                marks: Some("_".into()),
                attrs: HashMap::new(),
            },
        ),
        (
            "text".into(),
            NodeSpec {
                content: None,
                group: Some("inline".into()),
                inline: true,
                atom: false,
                marks: Some("_".into()),
                attrs: HashMap::new(),
            },
        ),
    ];

    let marks: Vec<(String, MarkSpec)> = vec![
        (
            "bold".into(),
            MarkSpec {
                group: None,
                inclusive: true,
                excludes: None,
                attrs: HashMap::new(),
            },
        ),
        (
            "italic".into(),
            MarkSpec {
                group: None,
                inclusive: true,
                excludes: None,
                attrs: HashMap::new(),
            },
        ),
        (
            "code".into(),
            MarkSpec {
                group: None,
                inclusive: false,
                excludes: Some("_".into()),
                attrs: HashMap::new(),
            },
        ),
        (
            "link".into(),
            MarkSpec {
                group: None,
                inclusive: false,
                excludes: None,
                attrs: {
                    let mut m = HashMap::new();
                    m.insert(
                        "href".into(),
                        AttrSpec {
                            default: Some(AttrValue::String("".into())),
                        },
                    );
                    m.insert(
                        "title".into(),
                        AttrSpec {
                            default: Some(AttrValue::Null),
                        },
                    );
                    m
                },
            },
        ),
    ];

    Arc::new(Schema::new(nodes, marks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_schema_nodes() {
        let s = basic_schema();
        assert!(s.node_type_by_name("doc").is_some());
        assert!(s.node_type_by_name("paragraph").is_some());
        assert!(s.node_type_by_name("heading").is_some());
        assert!(s.node_type_by_name("code_block").is_some());
        assert!(s.node_type_by_name("text").is_some());
    }

    #[test]
    fn basic_schema_marks() {
        let s = basic_schema();
        assert!(s.mark_type_by_name("bold").is_some());
        assert!(s.mark_type_by_name("italic").is_some());
        assert!(s.mark_type_by_name("code").is_some());
        assert!(s.mark_type_by_name("link").is_some());
    }

    #[test]
    fn top_node_is_doc() {
        let s = basic_schema();
        assert_eq!(s.node_type(s.top_node).name.as_ref(), "doc");
    }

    #[test]
    fn heading_default_attrs() {
        let s = basic_schema();
        let nt = s.node_type_by_name("heading").unwrap();
        let attrs = nt.default_attrs();
        assert_eq!(attrs.get("level"), Some(&AttrValue::Int(1)));
    }
}
