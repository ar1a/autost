use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    str,
    sync::{LazyLock, Mutex},
};

use html5ever::{
    interface::{ElementFlags, TreeSink},
    local_name, namespace_url, ns,
    tendril::{StrTendril, TendrilSink},
    tree_builder::TreeBuilderOpts,
    Attribute, LocalName, Namespace, ParseOpts, QualName,
};
use jane_eyre::eyre::{self, bail};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use serde_json::Value;
use tracing::{error, warn};

static ATTRIBUTES_SEEN: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());
static NOT_KNOWN_GOOD_ATTRIBUTES_SEEN: Mutex<BTreeSet<(String, String)>> =
    Mutex::new(BTreeSet::new());
static KNOWN_GOOD_ATTRIBUTES: LazyLock<BTreeSet<(Option<&'static str>, &'static str)>> =
    LazyLock::new(|| {
        let mut result = BTreeSet::default();
        result.insert((None, "aria-hidden"));
        result.insert((None, "aria-label"));
        result.insert((None, "id"));
        result.insert((None, "style"));
        result.insert((None, "tabindex"));
        result.insert((None, "title"));
        result.insert((Some("Mention"), "handle"));
        result.insert((Some("a"), "href"));
        result.insert((Some("a"), "name"));
        result.insert((Some("a"), "target"));
        result.insert((Some("details"), "name"));
        result.insert((Some("details"), "open"));
        result.insert((Some("div"), "align"));
        result.insert((Some("h3"), "align"));
        result.insert((Some("img"), "alt"));
        result.insert((Some("img"), "border"));
        result.insert((Some("img"), "height"));
        result.insert((Some("img"), "src"));
        result.insert((Some("img"), "width"));
        result.insert((Some("input"), "disabled"));
        result.insert((Some("input"), "name"));
        result.insert((Some("input"), "type"));
        result.insert((Some("input"), "value"));
        result.insert((Some("ol"), "start"));
        result.insert((Some("p"), "align"));
        result.insert((Some("td"), "align"));
        result.insert((Some("th"), "align"));
        result
    });
static RENAME_IDL_TO_CONTENT_ATTRIBUTE: LazyLock<
    BTreeMap<(Option<&'static str>, &'static str), &'static str>,
> = LazyLock::new(|| {
    let mut result = BTreeMap::default();
    result.insert((None, "ariaHidden"), "aria-hidden");
    result.insert((None, "ariaLabel"), "aria-label");
    result.insert((None, "className"), "class");
    result.insert((None, "tabIndex"), "tabindex");
    result
});

pub struct Traverse(Vec<Handle>);

impl Traverse {
    pub fn new(node: Handle) -> Self {
        Self(vec![node])
    }
}

impl Iterator for Traverse {
    type Item = Handle;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }

        let node = self.0.remove(0);
        for kid in node.children.borrow().iter() {
            self.0.push(kid.clone());
        }

        Some(node)
    }
}

pub fn make_html_tag_name(name: &str) -> QualName {
    QualName::new(None, ns!(html), LocalName::from(name))
}

pub fn make_attribute_name(name: &str) -> QualName {
    // per html5ever::Attribute docs:
    // “The namespace on the attribute name is almost always ns!(“”). The tokenizer creates all
    // attributes this way, but the tree builder will adjust certain attribute names inside foreign
    // content (MathML, SVG).”
    QualName::new(None, ns!(), LocalName::from(name))
}

pub fn parse(mut input: &[u8]) -> eyre::Result<RcDom> {
    let options = ParseOpts {
        tree_builder: TreeBuilderOpts {
            drop_doctype: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let context = QualName::new(None, ns!(html), local_name!("section"));
    let dom = html5ever::parse_fragment(RcDom::default(), options, context, vec![])
        .from_utf8()
        .read_from(&mut input)?;

    Ok(dom)
}

pub fn serialize(dom: RcDom) -> eyre::Result<String> {
    // html5ever::parse_fragment builds a tree with the input wrapped in an <html> element.
    // this is consistent with how the web platform dom requires exactly one root element.
    let children = dom.document.children.borrow();
    if children.len() != 1 {
        bail!(
            "expected exactly one root element but got {}",
            children.len()
        );
    }
    let html = QualName::new(None, ns!(html), local_name!("html"));
    if !matches!(&children[0].data, NodeData::Element { name, .. } if name == &html) {
        bail!("expected root element to be <html>");
    }
    let html_root: SerializableHandle = children[0].clone().into();

    let mut result = Vec::default();
    html5ever::serialize(&mut result, &html_root, Default::default())?;
    let result = String::from_utf8(result)?;

    Ok(result)
}

#[test]
fn test_serialize() -> eyre::Result<()> {
    assert_eq!(serialize(RcDom::default()).map_err(|_| ()), Err(()));

    let mut dom = RcDom::default();
    let html = create_element(&mut dom, "html");
    dom.document.children.borrow_mut().push(html);
    assert_eq!(serialize(dom)?, "");

    let mut dom = RcDom::default();
    let html = create_element(&mut dom, "html");
    dom.document.children.borrow_mut().push(html);
    let html = create_element(&mut dom, "html");
    dom.document.children.borrow_mut().push(html);
    assert_eq!(serialize(dom).map_err(|_| ()), Err(()));

    let mut dom = RcDom::default();
    let html = create_element(&mut dom, "p");
    dom.document.children.borrow_mut().push(html);
    assert_eq!(serialize(dom).map_err(|_| ()), Err(()));

    Ok(())
}

/// create a [`RcDom`] whose document has exactly one child, a wrapper <html> element.
pub fn create_fragment() -> (RcDom, Handle) {
    let mut dom = RcDom::default();
    let root = create_element(&mut dom, "html");
    dom.document.children.borrow_mut().push(root.clone());

    (dom, root)
}

pub fn create_element(dom: &mut RcDom, html_local_name: &str) -> Handle {
    let name = QualName::new(None, ns!(html), LocalName::from(html_local_name));
    dom.create_element(name, vec![], ElementFlags::default())
}

pub fn find_attr_mut<'attrs>(
    attrs: &'attrs mut [Attribute],
    name: &str,
) -> Option<&'attrs mut Attribute> {
    for attr in attrs.iter_mut() {
        if attr.name == QualName::new(None, Namespace::default(), LocalName::from(name)) {
            return Some(attr);
        }
    }

    None
}

pub fn attr_value<'attrs>(
    attrs: &'attrs [Attribute],
    name: &str,
) -> eyre::Result<Option<&'attrs str>> {
    for attr in attrs.iter() {
        if attr.name == QualName::new(None, Namespace::default(), LocalName::from(name)) {
            return Ok(Some(tendril_to_str(&attr.value)?));
        }
    }

    Ok(None)
}

pub fn tendril_to_str(tendril: &StrTendril) -> eyre::Result<&str> {
    Ok(str::from_utf8(tendril.borrow())?)
}

pub fn rename_idl_to_content_attribute(tag_name: &str, attribute_name: &str) -> QualName {
    let result = RENAME_IDL_TO_CONTENT_ATTRIBUTE
        .get_key_value(&(Some(tag_name), attribute_name))
        .or_else(|| RENAME_IDL_TO_CONTENT_ATTRIBUTE.get_key_value(&(None, attribute_name)))
        .map_or(attribute_name, |(_, name)| name);

    // to be extra cautious about converting attributes correctly, warn if we see attributes not on
    // our known-good list.
    ATTRIBUTES_SEEN
        .lock()
        .unwrap()
        .insert((tag_name.to_owned(), result.to_owned()));
    if !KNOWN_GOOD_ATTRIBUTES.contains(&(None, result))
        && !KNOWN_GOOD_ATTRIBUTES.contains(&(Some(tag_name), result))
    {
        warn!("saw attribute not on known-good-attributes list! check if output is correct for: <{tag_name} {result}>");
        NOT_KNOWN_GOOD_ATTRIBUTES_SEEN
            .lock()
            .unwrap()
            .insert((tag_name.to_owned(), result.to_owned()));
    }

    make_attribute_name(result)
}

#[test]

fn test_rename_idl_to_content_attribute() {
    assert_eq!(
        rename_idl_to_content_attribute("div", "tabIndex"),
        make_attribute_name("tabindex"),
    );
}

pub fn convert_idl_to_content_attribute(
    tag_name: &str,
    attribute_name: &str,
    value: Value,
) -> Option<Attribute> {
    if value == Value::Bool(false) {
        return None;
    }

    Some(Attribute {
        name: rename_idl_to_content_attribute(tag_name, attribute_name),
        value: match (tag_name, attribute_name, value) {
            (_, _, Value::String(value)) => value.into(),
            (_, _, Value::Number(value)) => value.to_string().into(),
            (_, _, Value::Bool(true)) => "".into(),
            (_, _, Value::Bool(false)) => return None,
            // idl arrays with space-separated content values.
            // TODO: is this correct?
            (_, "className" | "rel", Value::Array(values)) => values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .into(),
            (_, _, value) => {
                error!(
                    r"unknown attribute value type for <{tag_name} {attribute_name}>: {value:?}"
                );
                return None;
            }
        },
    })
}

#[test]
fn test_convert_idl_to_content_attribute() {
    assert_eq!(
        convert_idl_to_content_attribute("div", "id", Value::String("foo".to_owned())),
        Some(Attribute {
            name: make_attribute_name("id"),
            value: "foo".into(),
        }),
    );
    assert_eq!(
        convert_idl_to_content_attribute("img", "width", Value::Number(13.into())),
        Some(Attribute {
            name: make_attribute_name("width"),
            value: "13".into(),
        }),
    );
    assert_eq!(
        convert_idl_to_content_attribute("details", "open", Value::Bool(true)),
        Some(Attribute {
            name: make_attribute_name("open"),
            value: "".into(),
        }),
    );
    assert_eq!(
        convert_idl_to_content_attribute("details", "open", Value::Bool(false)),
        None,
    );
    assert_eq!(
        convert_idl_to_content_attribute(
            "div",
            "className",
            Value::Array(vec!["foo".into(), "bar".into()]),
        ),
        Some(Attribute {
            name: make_attribute_name("class"),
            value: "foo bar".into(),
        }),
    );
}

pub fn debug_attributes_seen() -> Vec<(String, String)> {
    ATTRIBUTES_SEEN.lock().unwrap().iter().cloned().collect()
}

pub fn debug_not_known_good_attributes_seen() -> Vec<(String, String)> {
    NOT_KNOWN_GOOD_ATTRIBUTES_SEEN
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}
