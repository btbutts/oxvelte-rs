//! `svelte/no-unknown-style-directive-property` — disallow unknown CSS properties in style directives.
//! ⭐ Recommended

use crate::ast::{Attribute, DirectiveKind, TemplateNode};
use crate::linter::{LintContext, Rule, walk_template_nodes};
use regex::{Regex, RegexBuilder};
use rustc_hash::FxHashSet;
use std::sync::OnceLock;

enum IgnorePattern {
    Exact(String),
    Regex(Regex),
}

impl IgnorePattern {
    fn from_config(value: &str) -> Self {
        if let Some((pattern, flags)) = parse_regex_literal(value) {
            let mut builder = RegexBuilder::new(pattern);
            builder.case_insensitive(flags.contains('i'));
            builder.multi_line(flags.contains('m'));
            builder.dot_matches_new_line(flags.contains('s'));
            if let Ok(regex) = builder.build() {
                return Self::Regex(regex);
            }
        }
        Self::Exact(value.to_string())
    }

    fn is_match(&self, prop: &str) -> bool {
        match self {
            Self::Exact(expected) => prop == expected,
            Self::Regex(regex) => regex.is_match(prop),
        }
    }
}

fn parse_regex_literal(value: &str) -> Option<(&str, &str)> {
    if !value.starts_with('/') {
        return None;
    }
    let end = value.rfind('/')?;
    if end == 0 {
        return None;
    }
    Some((&value[1..end], &value[end + 1..]))
}

#[derive(serde::Deserialize)]
struct KnownCssProperties {
    properties: Vec<String>,
}

fn known_css_properties() -> &'static FxHashSet<String> {
    static PROPERTIES: OnceLock<FxHashSet<String>> = OnceLock::new();
    PROPERTIES.get_or_init(|| {
        let data: KnownCssProperties =
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/known_css_properties.json")))
                .expect("Generated known_css_properties.json data should be valid JSON");
        data.properties.into_iter().collect()
    })
}

fn is_property_ignored(prop: &str, ignore_list: &[IgnorePattern]) -> bool {
    ignore_list.iter().any(|pat| pat.is_match(prop))
}

fn vendor_prefix(prop: &str) -> Option<&str> {
    let bytes = prop.as_bytes();
    if bytes.first() != Some(&b'-') {
        return None;
    }
    let mut end = 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    if end > 1 && bytes.get(end) == Some(&b'-') {
        Some(&prop[..=end])
    } else {
        None
    }
}

fn known_property(prop: &str) -> bool {
    if known_css_properties().contains(prop) {
        return true;
    }
    let Some(prefix) = vendor_prefix(prop) else {
        return false;
    };
    let unprefixed = &prop[prefix.len()..];
    matches!(prefix, "-moz-" | "-webkit-" | "-ms-" | "-o-") && known_css_properties().contains(unprefixed)
}

pub struct NoUnknownStyleDirectiveProperty;

impl Rule for NoUnknownStyleDirectiveProperty {
    fn name(&self) -> &'static str {
        "svelte/no-unknown-style-directive-property"
    }

    fn is_recommended(&self) -> bool {
        true
    }

    fn run<'a>(&self, ctx: &mut LintContext<'a>) {
        let option = ctx
            .config
            .options
            .as_ref()
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first());

        let ignore_properties: Vec<IgnorePattern> = option
            .and_then(|v| v.get("ignoreProperties"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(IgnorePattern::from_config))
                    .collect()
            })
            .unwrap_or_default();
        let ignore_prefixed = option
            .and_then(|v| v.get("ignorePrefixed"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        walk_template_nodes(&ctx.ast.html, &mut |node| {
            if let TemplateNode::Element(el) = node {
                for attr in &el.attributes {
                    if let Attribute::Directive {
                        kind: DirectiveKind::StyleDirective,
                        name,
                        span,
                        ..
                    } = attr
                    {
                        if name.starts_with("--") {
                            continue;
                        }
                        if is_property_ignored(name, &ignore_properties) {
                            continue;
                        }
                        if ignore_prefixed && vendor_prefix(name).is_some() {
                            continue;
                        }
                        if !known_property(name) {
                            ctx.diagnostic(
                                format!("Unexpected unknown style directive property '{}'.", name),
                                oxc::span::Span::new(span.start + 6, span.start + 6 + name.len() as u32),
                            );
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod span_tests {
    use super::*;
    #[test]
    fn reports_only_the_property_name_after_unicode() {
        let source = "<div title='é' style:unknown-color={value}/>";
        let allocator = oxc::allocator::Allocator::default();
        let parsed = crate::parser::parse_for_lint(source, &allocator);
        let mut ctx = LintContext::new(&parsed.ast, source);
        NoUnknownStyleDirectiveProperty.run(&mut ctx);
        let diagnostics = ctx.into_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        let span = diagnostics[0].span;
        assert_eq!(&source[span.start as usize..span.end as usize], "unknown-color");
    }
}
