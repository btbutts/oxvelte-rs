//! The `no-unnecessary-condition` analysis ported from
//! `typescript-conditions.cjs`, driven through a `TsrsSession` instead of the
//! JS sidecar. The virtual file is walked with oxc's TypeScript AST; tsrs
//! answers only type and symbol queries. Type flags are TS7's `TypeFlags`
//! (renumbered from TS <= 5 — see the planning doc).
//!
//! oxc's `Visit` is infallible, so checks accumulate into `self.error` and
//! the walk becomes a no-op once one is set; `analyze` surfaces it.

use std::collections::HashSet;

use oxc::ast::ast::*;
use oxc::ast_visit::{Visit, walk};
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType, Span};
use oxc::syntax::scope::ScopeFlags;

use crate::compiler::byte_utf16;
use crate::tsrs::TsrsError;
use crate::tsrs::protocol::TypeResponse;
use crate::tsrs::session::TsrsSession;

// TS7 TypeFlags (tsc/internal/checker/types.go at the tsrs pin).
const ANY: u32 = 1 << 0;
const UNKNOWN: u32 = 1 << 1;
const UNDEFINED: u32 = 1 << 2;
const NULL: u32 = 1 << 3;
const VOID: u32 = 1 << 4;
const STRING: u32 = 1 << 5;
const NUMBER: u32 = 1 << 6;
const BIGINT: u32 = 1 << 7;
const BOOLEAN: u32 = 1 << 8;
const STRING_LITERAL: u32 = 1 << 10;
const NUMBER_LITERAL: u32 = 1 << 11;
const BIGINT_LITERAL: u32 = 1 << 12;
const BOOLEAN_LITERAL: u32 = 1 << 13;
const NEVER: u32 = 1 << 18;
const TYPE_PARAMETER: u32 = 1 << 19;
const UNION: u32 = 1 << 27;
const LITERAL: u32 = STRING_LITERAL | NUMBER_LITERAL | BIGINT_LITERAL | BOOLEAN_LITERAL;
const DEFINITELY_FALSY: u32 =
    STRING_LITERAL | NUMBER_LITERAL | BIGINT_LITERAL | BOOLEAN_LITERAL | VOID | UNDEFINED | NULL;
const POSSIBLY_FALSY: u32 = DEFINITELY_FALSY | STRING | NUMBER | BIGINT | BOOLEAN;

const MESSAGES: [(&str, &str); 11] = [
    ("alwaysTruthy", "Unnecessary conditional, value is always truthy."),
    ("alwaysFalsy", "Unnecessary conditional, value is always falsy."),
    (
        "alwaysTruthyFunc",
        "This callback should return a conditional, but return is always truthy.",
    ),
    (
        "alwaysFalsyFunc",
        "This callback should return a conditional, but return is always falsy.",
    ),
    (
        "neverNullish",
        "Unnecessary conditional, expected left-hand side of `??` operator to be possibly null or undefined.",
    ),
    (
        "alwaysNullish",
        "Unnecessary conditional, left-hand side of `??` operator is always `null` or `undefined`.",
    ),
    (
        "literalBooleanExpression",
        "Unnecessary conditional, both sides of the expression are literal values.",
    ),
    (
        "noOverlapBooleanExpression",
        "Unnecessary conditional, the types have no overlap.",
    ),
    ("never", "Unnecessary conditional, value is `never`."),
    (
        "neverOptionalChain",
        "Unnecessary optional chain on a non-nullish value.",
    ),
    (
        "noStrictNullCheck",
        "This rule requires the `strictNullChecks` compiler option to be turned on to function correctly.",
    ),
];

fn message(id: &str) -> &'static str {
    MESSAGES
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, text)| *text)
        .expect("unknown message id")
}

/// One diagnostic in original-component UTF-16 offsets, matching the JS
/// sidecar's response shape.
#[derive(Debug, Clone)]
pub struct ConditionDiagnostic {
    pub message: String,
    pub start: usize,
    pub end: usize,
    pub fix: Option<ConditionFix>,
}

#[derive(Debug, Clone)]
pub struct ConditionFix {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub fn analyze(session: &mut TsrsSession, options: &serde_json::Value) -> Result<Vec<ConditionDiagnostic>, TsrsError> {
    let allocator = oxc::allocator::Allocator::default();
    let virtual_source = session.virtual_source.clone();
    let parsed = Parser::new(&allocator, &virtual_source, SourceType::ts()).parse();
    if !parsed.diagnostics.is_empty() {
        let details = parsed
            .diagnostics
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(TsrsError::Server {
            method: "parse".to_owned(),
            message: format!("Unable to parse component TypeScript: {details}"),
        });
    }
    let syntax =
        session
            .client
            .get_syntactic_diagnostics(session.snapshot, &session.project, &session.virtual_filename)?;
    if !syntax.is_empty() {
        return Err(TsrsError::Server {
            method: "getSyntacticDiagnostics".to_owned(),
            message: format!(
                "Unable to parse component TypeScript: {}",
                syntax.iter().map(|d| d.text.as_str()).collect::<Vec<_>>().join("\n")
            ),
        });
    }

    let mut analyzer = Analyzer {
        session,
        diagnostics: Vec::new(),
        mutable_roots: HashSet::new(),
        top_level_reactive: false,
        allow_without_strict_null: options
            .get("allowRuleToRunWithoutStrictNullChecksIKnowWhatIAmDoing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        allow_constant_loop_conditions: options
            .get("allowConstantLoopConditions")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        error: None,
    };
    analyzer.collect_mutable_roots(&parsed.program)?;
    if !analyzer.session.strict_null_checks && !analyzer.allow_without_strict_null {
        analyzer.diagnostics.push(ConditionDiagnostic {
            message: message("noStrictNullCheck").to_owned(),
            start: 0,
            end: 0,
            fix: None,
        });
    }
    for statement in &parsed.program.body {
        analyzer.top_level_reactive =
            matches!(statement, Statement::LabeledStatement(labeled) if labeled.label.name == "$");
        analyzer.visit_statement(statement);
        analyzer.top_level_reactive = false;
    }
    if let Some(error) = analyzer.error {
        return Err(error);
    }
    analyzer.diagnostics.sort_by_key(|d| (d.start, d.end));
    Ok(analyzer.diagnostics)
}

struct Analyzer<'s> {
    session: &'s mut TsrsSession,
    diagnostics: Vec<ConditionDiagnostic>,
    mutable_roots: HashSet<u64>,
    top_level_reactive: bool,
    allow_without_strict_null: bool,
    allow_constant_loop_conditions: bool,
    error: Option<TsrsError>,
}

impl Analyzer<'_> {
    fn utf16_span(&self, span: Span) -> (usize, usize) {
        (
            byte_utf16(&self.session.virtual_source, span.start),
            byte_utf16(&self.session.virtual_source, span.end),
        )
    }

    fn handle(&mut self, span: Span) -> Result<Option<String>, TsrsError> {
        let (start, end) = self.utf16_span(span);
        self.session.handle_at_span(start, end)
    }

    fn type_of(&mut self, expression: &Expression) -> Result<Option<TypeResponse>, TsrsError> {
        let expression = unwrap(expression);
        let (start, end) = self.utf16_span(expression.span());
        self.session.type_at_span(start, end)
    }

    /// `parts`: recursively flattened union parts.
    fn parts(&mut self, ty: &TypeResponse) -> Result<Vec<TypeResponse>, TsrsError> {
        if ty.flags & UNION != 0 {
            let parts = self
                .session
                .client
                .get_types_of_type(self.session.snapshot, &self.session.project, ty.id)?;
            let mut flattened = Vec::new();
            for part in &parts {
                flattened.extend(self.parts(part)?);
            }
            Ok(flattened)
        } else {
            Ok(vec![ty.clone()])
        }
    }

    fn has(&mut self, ty: &TypeResponse, flags: u32) -> Result<bool, TsrsError> {
        Ok(self.parts(ty)?.iter().any(|part| part.flags & flags != 0))
    }

    fn uncertain(&mut self, ty: &Option<TypeResponse>) -> Result<bool, TsrsError> {
        match ty {
            None => Ok(true),
            Some(ty) => self.has(ty, ANY | UNKNOWN | TYPE_PARAMETER),
        }
    }

    fn nullish(&mut self, ty: &TypeResponse) -> Result<bool, TsrsError> {
        self.has(ty, NULL | UNDEFINED)
    }

    fn possibly_falsy(&mut self, ty: &TypeResponse) -> Result<bool, TsrsError> {
        Ok(self
            .parts(ty)?
            .iter()
            .any(|part| !truthy_literal(part) && part.flags & POSSIBLY_FALSY != 0))
    }

    fn possibly_truthy(&mut self, ty: &TypeResponse) -> Result<bool, TsrsError> {
        Ok(self.parts(ty)?.iter().any(|part| {
            part.flags & NEVER == 0
                && !(part.flags & (NULL | UNDEFINED | VOID) != 0
                    || (part.flags & BOOLEAN_LITERAL != 0 && part.value.as_bool() == Some(false))
                    || (part.flags & (STRING_LITERAL | NUMBER_LITERAL) != 0 && !literal_value_truthy(&part.value))
                    || (part.flags & BIGINT_LITERAL != 0 && part.value.as_str() == Some("0")))
        }))
    }

    fn report(&mut self, span: Span, id: &str) {
        let (start, end) = self.utf16_span(span);
        if let Some((start, end)) = self.session.project_range(start, end) {
            self.diagnostics.push(ConditionDiagnostic {
                message: message(id).to_owned(),
                start,
                end,
                fix: None,
            });
        }
    }

    /// The Svelte exclusion: nodes in template regions or top-level `$:`
    /// statements that reference mutable root bindings are skipped, because
    /// Svelte reactivity makes those types flow-sensitive in ways the checker
    /// can't see.
    fn is_reactive(&mut self, expression: &Expression) -> Result<bool, TsrsError> {
        self.is_reactive_with(expression.span(), |walker| walker.visit_expression(expression))
    }

    /// The same check for a binary comparison node (walked by its operands).
    fn is_reactive_binary(&mut self, binary: &BinaryExpression) -> Result<bool, TsrsError> {
        self.is_reactive_with(binary.span, |walker| {
            walker.visit_expression(&binary.left);
            walker.visit_expression(&binary.right);
        })
    }

    fn is_reactive_with(
        &mut self,
        span: Span,
        walk: impl FnOnce(&mut MutableReferenceWalker),
    ) -> Result<bool, TsrsError> {
        let (start, _) = self.utf16_span(span);
        let in_template = start >= self.session.source_len_utf16;
        if !in_template && !self.top_level_reactive {
            return Ok(false);
        }
        let mut walker = MutableReferenceWalker {
            session: &mut *self.session,
            roots: &self.mutable_roots,
            found: false,
            error: None,
        };
        walk(&mut walker);
        if let Some(error) = walker.error {
            return Err(error);
        }
        Ok(walker.found)
    }

    fn collect_mutable_roots(&mut self, program: &Program) -> Result<(), TsrsError> {
        // Top-level non-const variable statements.
        for statement in &program.body {
            // oxc wraps exported declarations in ExportNamedDeclaration, while
            // TS models `export let x` as a VariableStatement with modifiers —
            // both must feed the mutable-root set.
            let declaration = match statement {
                Statement::VariableDeclaration(declaration) => Some(&**declaration),
                Statement::ExportDeclaration(export) => match &export.declaration {
                    Declaration::VariableDeclaration(declaration) => Some(declaration.as_ref()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(declaration) = declaration {
                if declaration.kind != VariableDeclarationKind::Const {
                    for declarator in &declaration.declarations {
                        collect_binding_symbols(self.session, &declarator.id, &mut self.mutable_roots)?;
                    }
                }
            }
        }
        // Hoisted `var` declarations anywhere outside function/class scopes.
        let mut hoisted = HoistedVarCollector {
            session: &mut *self.session,
            roots: &mut self.mutable_roots,
            error: None,
        };
        hoisted.visit_program(program);
        if let Some(error) = hoisted.error {
            return Err(error);
        }
        Ok(())
    }

    /// `arrayIndex`: element access into an array/tuple (with a non-literal
    /// index for tuples).
    fn array_index(&mut self, expression: &Expression) -> Result<bool, TsrsError> {
        let expression = unwrap(expression);
        let Expression::ComputedMemberExpression(member) = expression else {
            return Ok(false);
        };
        let Some(object_type) = self.type_of(&member.object)? else {
            return Ok(false);
        };
        if self.is_array_type(&object_type)? {
            return Ok(true);
        }
        Ok(object_type.is_tuple_type && !is_literal_expression(unwrap(&member.expression)))
    }

    fn is_array_type(&mut self, ty: &TypeResponse) -> Result<bool, TsrsError> {
        self.session
            .client
            .is_array_type(self.session.snapshot, &self.session.project, ty.id)
    }

    /// `optionalArrayIndex`: an optional chain whose base is an array index,
    /// recursing through the chain's object expressions.
    fn optional_array_index_parts(&mut self, object: &Expression, optional: bool) -> Result<bool, TsrsError> {
        if optional && self.array_index(object)? {
            return Ok(true);
        }
        match unwrap(object) {
            Expression::StaticMemberExpression(member) => {
                self.optional_array_index_parts(&member.object, member.optional)
            }
            Expression::ComputedMemberExpression(member) => {
                self.optional_array_index_parts(&member.object, member.optional)
            }
            Expression::CallExpression(call) => self.optional_array_index_parts(&call.callee, call.optional),
            _ => Ok(false),
        }
    }

    /// `optionalArrayIndex` on a whole expression (the `checkNullish` caller).
    fn optional_array_index(&mut self, expression: &Expression) -> Result<bool, TsrsError> {
        match unwrap(expression) {
            Expression::StaticMemberExpression(member) => {
                self.optional_array_index_parts(&member.object, member.optional)
            }
            Expression::ComputedMemberExpression(member) => {
                self.optional_array_index_parts(&member.object, member.optional)
            }
            Expression::CallExpression(call) => self.optional_array_index_parts(&call.callee, call.optional),
            _ => Ok(false),
        }
    }

    fn check(&mut self, expression: &Expression, inverted: bool) -> Result<(), TsrsError> {
        let expression = unwrap(expression);
        if self.is_reactive(expression)? {
            return Ok(());
        }
        if let Expression::UnaryExpression(unary) = expression {
            if unary.operator == UnaryOperator::LogicalNot {
                return self.check(&unary.argument, true);
            }
        }
        if self.array_index(expression)? {
            return Ok(());
        }
        if let Expression::LogicalExpression(logical) = expression {
            return self.check(&logical.right, false);
        }
        let ty = self.type_of(expression)?;
        if self.uncertain(&ty)? {
            return Ok(());
        }
        let ty = ty.expect("uncertain covers None");
        if self.has(&ty, NEVER)? {
            self.report(expression.span(), "never");
        } else if !self.possibly_truthy(&ty)? {
            self.report(expression.span(), if inverted { "alwaysTruthy" } else { "alwaysFalsy" });
        } else if !self.possibly_falsy(&ty)? {
            self.report(expression.span(), if inverted { "alwaysFalsy" } else { "alwaysTruthy" });
        }
        Ok(())
    }

    fn check_nullish(&mut self, expression: &Expression) -> Result<(), TsrsError> {
        let expression = unwrap(expression);
        if self.is_reactive(expression)? {
            return Ok(());
        }
        let ty = self.type_of(expression)?;
        let Some(ty) = ty else { return Ok(()) };
        if self.has(&ty, ANY | UNKNOWN)? {
            return Ok(());
        }
        if self.has(&ty, NEVER)? {
            self.report(expression.span(), "never");
        } else if !self.nullish(&ty)? {
            if !self.array_index(expression)? && !self.optional_array_index(expression)? {
                self.report(expression.span(), "neverNullish");
            }
        } else {
            let all_nullish = self.parts(&ty)?.iter().all(|part| part.flags & (NULL | UNDEFINED) != 0);
            if all_nullish {
                self.report(expression.span(), "alwaysNullish");
            }
        }
        Ok(())
    }

    fn check_comparison(&mut self, binary: &BinaryExpression) -> Result<(), TsrsError> {
        if self.is_reactive_binary(binary)? {
            return Ok(());
        }
        let operator = binary.operator.as_str();
        if !["<", ">", "<=", ">=", "==", "===", "!=", "!=="].contains(&operator) {
            return Ok(());
        }
        let (Some(left), Some(right)) = (self.type_of(&binary.left)?, self.type_of(&binary.right)?) else {
            return Ok(());
        };
        if is_literal_type(&left) && is_literal_type(&right) {
            self.report(binary.span, "literalBooleanExpression");
            return Ok(());
        }
        if !self.session.strict_null_checks {
            return Ok(());
        }
        for flag in [NULL, UNDEFINED] {
            let loose = operator == "==" || operator == "!=";
            if (left.flags == flag && !self.comparable(&right, flag, loose)?)
                || (right.flags == flag && !self.comparable(&left, flag, loose)?)
            {
                self.report(binary.span, "noOverlapBooleanExpression");
                return Ok(());
            }
        }
        Ok(())
    }

    fn comparable(&mut self, ty: &TypeResponse, flag: u32, loose: bool) -> Result<bool, TsrsError> {
        self.has(
            ty,
            flag | ANY | UNKNOWN | TYPE_PARAMETER | if loose { NULL | UNDEFINED } else { 0 },
        )
    }

    fn property_nullable(
        &mut self,
        object_type: &TypeResponse,
        property_type: &TypeResponse,
        at: Span,
    ) -> Result<bool, TsrsError> {
        let Some(at_handle) = self.handle(at)? else {
            return Ok(false);
        };
        for part in self.parts(property_type)? {
            if part.flags & (STRING_LITERAL | NUMBER_LITERAL) != 0 {
                let name = match &part.value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                let property = self.session.client.get_property_of_type(
                    self.session.snapshot,
                    &self.session.project,
                    object_type.id,
                    &name,
                )?;
                if let Some(property) = property {
                    let property_type = self.session.client.get_type_of_symbol_at_location(
                        self.session.snapshot,
                        &self.session.project,
                        &property.reference,
                        &at_handle,
                    )?;
                    if let Some(property_type) = property_type {
                        if self.nullish(&property_type)? {
                            return Ok(true);
                        }
                    }
                }
            } else if part.flags & STRING != 0 {
                let info = self.session.client.get_index_info_of_type(
                    self.session.snapshot,
                    &self.session.project,
                    object_type.id,
                    0,
                )?;
                if info.is_some() {
                    return Ok(true);
                }
            } else if part.flags & NUMBER != 0 {
                let info = self.session.client.get_index_info_of_type(
                    self.session.snapshot,
                    &self.session.project,
                    object_type.id,
                    1,
                )?;
                if info.is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn nullable_from_previous(&mut self, expression: &Expression) -> Result<bool, TsrsError> {
        let (object, computed_argument) = match unwrap(expression) {
            Expression::StaticMemberExpression(member) => (&member.object, None),
            Expression::ComputedMemberExpression(member) => (&member.object, Some(&member.expression)),
            _ => return Ok(false),
        };
        let Some(previous) = self.type_of(object)? else {
            return Ok(false);
        };
        if previous.flags & UNION == 0 {
            return Ok(false);
        }
        if let Some(argument) = computed_argument {
            if !matches!(unwrap(argument), Expression::Identifier(_)) {
                return Ok(false);
            }
        }
        let mut own_nullable = false;
        for part in self.parts(&previous)? {
            let nullable = if let Some(argument) = computed_argument {
                let Some(argument_type) = self.type_of(argument)? else {
                    continue;
                };
                self.property_nullable(&part, &argument_type, expression.span())?
            } else {
                let Expression::StaticMemberExpression(member) = unwrap(expression) else {
                    continue;
                };
                let property = self.session.client.get_property_of_type(
                    self.session.snapshot,
                    &self.session.project,
                    part.id,
                    member.property.name.as_str(),
                )?;
                match property {
                    Some(property) => {
                        let Some(at_handle) = self.handle(expression.span())? else {
                            continue;
                        };
                        let property_type = self.session.client.get_type_of_symbol_at_location(
                            self.session.snapshot,
                            &self.session.project,
                            &property.reference,
                            &at_handle,
                        )?;
                        match property_type {
                            Some(property_type) => self.nullish(&property_type)?,
                            None => false,
                        }
                    }
                    None => false,
                }
            };
            if nullable {
                own_nullable = true;
                break;
            }
        }
        Ok(!own_nullable && self.nullish(&previous)?)
    }

    /// The optional-chain check, parameterized by parts so it serves both
    /// plain expressions and oxc's chain-element member/call nodes.
    fn check_optional_parts(
        &mut self,
        object: &Expression,
        optional: bool,
        static_property: Option<Span>,
        whole_span: Span,
    ) -> Result<(), TsrsError> {
        if !optional || self.optional_array_index_parts(object, optional)? {
            return Ok(());
        }
        let object = unwrap(object);
        if self.is_reactive(object)? {
            return Ok(());
        }
        let ty = self.type_of(object)?;
        let Some(ty) = ty else { return Ok(()) };
        if self.has(&ty, ANY | UNKNOWN)? {
            return Ok(());
        }
        if self.nullish(&ty)? && !self.nullable_from_previous(object)? {
            return Ok(());
        }
        // The questionDotToken: `?.` between the object and the property.
        let source = self.session.virtual_source.as_str();
        let object_end = object.span().end as usize;
        let search_end = static_property.map_or(whole_span.end as usize, |s| s.start as usize);
        let Some(relative) = source[object_end..search_end].find("?.") else {
            return Ok(());
        };
        let dot_start = object_end + relative;
        let (start, end) = self.utf16_span(Span::new(dot_start as u32, (dot_start + 2) as u32));
        let Some((start, end)) = self.session.project_range(start, end) else {
            return Ok(());
        };
        let text = if static_property.is_some() { "." } else { "" }.to_owned();
        self.diagnostics.push(ConditionDiagnostic {
            message: message("neverOptionalChain").to_owned(),
            start,
            end,
            fix: Some(ConditionFix { start, end, text }),
        });
        Ok(())
    }

    fn check_predicate(&mut self, call: &CallExpression) -> Result<(), TsrsError> {
        let callee = unwrap(&call.callee);
        let Expression::StaticMemberExpression(member) = callee else {
            return Ok(());
        };
        if !["filter", "find", "some", "every"].contains(&member.property.name.as_str()) {
            return Ok(());
        }
        let Some(object_type) = self.type_of(&member.object)? else {
            return Ok(());
        };
        if !self.is_array_type(&object_type)? && !object_type.is_tuple_type {
            return Ok(());
        }
        let Some(argument) = call.arguments.first() else {
            return Ok(());
        };
        let Some(callback_expression) = argument.as_expression() else {
            return Ok(());
        };
        let callback = unwrap(callback_expression);
        match callback {
            Expression::ArrowFunctionExpression(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(body) => {
                    if body.statements.len() == 1 {
                        if let Some(Statement::ReturnStatement(statement)) = body.statements.first() {
                            if let Some(expression) = &statement.argument {
                                return self.check(expression, false);
                            }
                        }
                    }
                }
                expression => {
                    if let Some(expression) = expression.as_expression() {
                        return self.check(expression, false);
                    }
                }
            },
            Expression::FunctionExpression(function) => {
                if let Some(body) = &function.body {
                    if body.statements.len() == 1 {
                        if let Some(Statement::ReturnStatement(statement)) = body.statements.first() {
                            if let Some(expression) = &statement.argument {
                                return self.check(expression, false);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let Some(callback_type) = self.type_of(callback)? else {
            return Ok(());
        };
        let signatures = self.session.client.get_signatures_of_type(
            self.session.snapshot,
            &self.session.project,
            callback_type.id,
            0,
        )?;
        let mut returns = Vec::new();
        for signature in signatures {
            let return_type = self.session.client.get_return_type_of_signature(
                self.session.snapshot,
                &self.session.project,
                signature.id,
            )?;
            if let Some(return_type) = return_type {
                returns.push(return_type);
            }
        }
        if returns.is_empty() {
            return Ok(());
        }
        for ty in &returns {
            if self.has(ty, ANY | UNKNOWN)? {
                return Ok(());
            }
        }
        let mut any_possibly_falsy = false;
        let mut any_possibly_truthy = false;
        for ty in &returns {
            any_possibly_falsy |= self.possibly_falsy(ty)?;
            any_possibly_truthy |= self.possibly_truthy(ty)?;
        }
        if !any_possibly_falsy {
            self.report(callback.span(), "alwaysTruthyFunc");
        } else if !any_possibly_truthy {
            self.report(callback.span(), "alwaysFalsyFunc");
        }
        Ok(())
    }

    fn check_loop_condition(&mut self, test: &Expression) -> Result<(), TsrsError> {
        if self.allow_constant_loop_conditions {
            if let Some(ty) = self.type_of(test)? {
                if ty.flags & BOOLEAN_LITERAL != 0 && ty.value.as_bool() == Some(true) {
                    return Ok(());
                }
            }
        }
        self.check(test, false)
    }

    fn visit_expression_checked(&mut self, expression: &Expression) -> Result<(), TsrsError> {
        match expression {
            Expression::LogicalExpression(logical) => match logical.operator {
                LogicalOperator::And | LogicalOperator::Or => self.check(&logical.left, false)?,
                LogicalOperator::Coalesce => self.check_nullish(&logical.left)?,
            },
            Expression::BinaryExpression(binary) => self.check_comparison(binary)?,
            Expression::ConditionalExpression(conditional) => {
                self.check(&conditional.test, false)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_statement_checked(&mut self, statement: &Statement) -> Result<(), TsrsError> {
        match statement {
            Statement::IfStatement(if_statement) => {
                self.check(&if_statement.test, false)?;
            }
            Statement::WhileStatement(while_statement) => {
                self.check_loop_condition(&while_statement.test)?;
            }
            Statement::DoWhileStatement(do_while) => {
                self.check_loop_condition(&do_while.test)?;
            }
            Statement::ForStatement(for_statement) => {
                if let Some(test) = &for_statement.test {
                    self.check_loop_condition(test)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl<'a> Visit<'a> for Analyzer<'_> {
    fn visit_statement(&mut self, statement: &Statement<'a>) {
        if self.error.is_none() {
            if let Err(error) = self.visit_statement_checked(statement) {
                self.error = Some(error);
            }
        }
        if self.error.is_none() {
            walk::walk_statement(self, statement);
        }
    }

    fn visit_expression(&mut self, expression: &Expression<'a>) {
        if self.error.is_none() {
            if let Err(error) = self.visit_expression_checked(expression) {
                self.error = Some(error);
            }
        }
        if self.error.is_none() {
            walk::walk_expression(self, expression);
        }
    }

    /// Calls are visited here rather than in `visit_expression` because chain
    /// elements route straight to `visit_call_expression`.
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if self.error.is_none() {
            let result = self
                .check_predicate(call)
                .and_then(|()| self.check_optional_parts(&call.callee, call.optional, None, call.span));
            if let Err(error) = result {
                self.error = Some(error);
            }
        }
        if self.error.is_none() {
            walk::walk_call_expression(self, call);
        }
    }

    /// Member expressions are visited here rather than in `visit_expression`:
    /// oxc routes both plain and chain-element members through this method.
    fn visit_member_expression(&mut self, member: &MemberExpression<'a>) {
        if self.error.is_none() {
            let result = match member {
                MemberExpression::StaticMemberExpression(member) => {
                    self.check_optional_parts(&member.object, member.optional, Some(member.property.span), member.span)
                }
                MemberExpression::ComputedMemberExpression(member) => {
                    self.check_optional_parts(&member.object, member.optional, None, member.span)
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                self.error = Some(error);
            }
        }
        if self.error.is_none() {
            walk::walk_member_expression(self, member);
        }
    }
}

/// Unwraps parenthesized expressions (the sidecar's `unwrap`). Chain
/// expressions are intentionally not unwrapped: spans are what the session
/// resolves, and the walker visits the inner optional members directly.
fn unwrap<'a>(expression: &'a Expression) -> &'a Expression<'a> {
    match expression {
        Expression::ParenthesizedExpression(paren) => unwrap(&paren.expression),
        _ => expression,
    }
}

fn truthy_literal(ty: &TypeResponse) -> bool {
    if ty.flags & BOOLEAN_LITERAL != 0 {
        return ty.value.as_bool() == Some(true);
    }
    if ty.flags & (STRING_LITERAL | NUMBER_LITERAL) != 0 {
        return literal_value_truthy(&ty.value);
    }
    if ty.flags & BIGINT_LITERAL != 0 {
        return ty.value.as_str().is_some_and(|value| value != "0");
    }
    false
}

fn literal_value_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => false,
    }
}

/// The sidecar's `literal`: boolean literal, null, undefined, or any literal.
fn is_literal_type(ty: &TypeResponse) -> bool {
    ty.flags & (BOOLEAN_LITERAL | NULL | UNDEFINED) != 0 || ty.flags & LITERAL != 0
}

fn is_literal_expression(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::RegExpLiteral(_)
    )
}

/// Records the symbol of every binding identifier in a pattern, including
/// destructuring and defaults, into `roots`.
fn collect_binding_symbols(
    session: &mut TsrsSession,
    pattern: &BindingPattern,
    roots: &mut HashSet<u64>,
) -> Result<(), TsrsError> {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            let start = byte_utf16(&session.virtual_source, identifier.span.start);
            let end = byte_utf16(&session.virtual_source, identifier.span.end);
            if let Some(handle) = session.handle_at_span(start, end)? {
                let symbol = session
                    .client
                    .get_symbol_at_location(session.snapshot, &session.project, &handle)?;
                if let Some(symbol) = symbol {
                    roots.insert(symbol.reference.id);
                }
            }
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_binding_symbols(session, &property.value, roots)?;
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_symbols(session, &rest.argument, roots)?;
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_binding_symbols(session, element, roots)?;
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_symbols(session, &rest.argument, roots)?;
            }
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_binding_symbols(session, &pattern.left, roots)?;
        }
    }
    Ok(())
}

/// Collects hoisted `var` bindings, stopping at function-like and class-like
/// scopes (the sidecar's `collectHoisted`).
struct HoistedVarCollector<'s> {
    session: &'s mut TsrsSession,
    roots: &'s mut HashSet<u64>,
    error: Option<TsrsError>,
}

impl<'a> Visit<'a> for HoistedVarCollector<'_> {
    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        if self.error.is_some() || declaration.kind != VariableDeclarationKind::Var {
            return;
        }
        for declarator in &declaration.declarations {
            if let Err(error) = collect_binding_symbols(self.session, &declarator.id, self.roots) {
                self.error = Some(error);
                return;
            }
        }
    }

    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _arrow: &ArrowFunctionExpression<'a>) {}

    fn visit_class(&mut self, _class: &Class<'a>) {}
}

/// Walks an expression for identifier references and reports whether any
/// resolve to a mutable root symbol (the sidecar's `isReactive` reference
/// scan, including the shorthand-property value-symbol special case).
struct MutableReferenceWalker<'s> {
    session: &'s mut TsrsSession,
    roots: &'s HashSet<u64>,
    found: bool,
    error: Option<TsrsError>,
}

impl MutableReferenceWalker<'_> {
    fn query_symbol(&mut self, span: Span, shorthand: bool) {
        if self.found || self.error.is_some() {
            return;
        }
        let result = (|| -> Result<(), TsrsError> {
            let start = byte_utf16(&self.session.virtual_source, span.start);
            let end = byte_utf16(&self.session.virtual_source, span.end);
            let Some(handle) = self.session.handle_at_span(start, end)? else {
                return Ok(());
            };
            let symbol = if shorthand {
                self.session.client.get_shorthand_assignment_value_symbol(
                    self.session.snapshot,
                    &self.session.project,
                    &handle,
                )?
            } else {
                self.session
                    .client
                    .get_symbol_at_location(self.session.snapshot, &self.session.project, &handle)?
            };
            if let Some(symbol) = symbol {
                if self.roots.contains(&symbol.reference.id) {
                    self.found = true;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
        }
    }
}

impl<'a> Visit<'a> for MutableReferenceWalker<'_> {
    fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
        self.query_symbol(identifier.span, false);
    }

    fn visit_identifier_name(&mut self, identifier: &IdentifierName<'a>) {
        self.query_symbol(identifier.span, false);
    }

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        if property.shorthand {
            // The sidecar resolves the shorthand's value symbol rather than
            // the identifier's own symbol.
            self.query_symbol(property.span, true);
            return;
        }
        walk::walk_object_property(self, property);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tsrs::session::TemplateExpression;
    use std::fs;

    /// Runs the full analysis over a component with known unnecessary
    /// conditions and asserts messages, projected ranges, and the optional
    /// chain fix. Skips when no tsrs binary is on PATH.
    #[test]
    fn analyze_finds_unnecessary_conditions() {
        if crate::tsrs::ts_locator::find_on_path().is_none() {
            eprintln!("skipping: no tsrs binary on PATH");
            return;
        }
        let dir = std::env::temp_dir().join("oxvelte-tsrs-conditions-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();

        let source = concat!(
            "<script lang=\"ts\">\n",
            "const definitely = true;\n",
            "const obj = { a: 1 };\n",
            "const v = obj?.a;\n",
            "</script>\n",
            "{#if definitely}<p>hi</p>{/if}\n",
            "<p>{definitely ?? \"fallback\"}</p>\n"
        );
        let script_start = source.find("const definitely").unwrap();
        let script_end = source.find("const v = obj?.a;").unwrap() + "const v = obj?.a;".len();
        let if_test = source.find("{#if definitely}").unwrap() + "{#if ".len();
        let nullish = source.find("{definitely ??").unwrap() + 1;

        let mut session = TsrsSession::create(
            &dir.join("Component.svelte"),
            source,
            &[(script_start, script_end)],
            &[
                TemplateExpression {
                    start: if_test,
                    end: if_test + "definitely".len(),
                    condition: true,
                },
                TemplateExpression {
                    start: nullish,
                    end: nullish + "definitely ?? \"fallback\"".len(),
                    condition: false,
                },
            ],
            &serde_json::json!({}),
        )
        .expect("session");

        let diagnostics = analyze(&mut session, &serde_json::json!({})).expect("analyze");
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(
            messages.contains(&message("alwaysTruthy")),
            "if test should be always truthy: {messages:?}"
        );
        assert!(
            messages.contains(&message("neverNullish")),
            "?? left should be never nullish: {messages:?}"
        );
        assert!(
            messages.contains(&message("neverOptionalChain")),
            "optional chain should be unnecessary: {messages:?}"
        );

        // Ranges project back to the original component source.
        let always_truthy = diagnostics
            .iter()
            .find(|d| d.message == message("alwaysTruthy"))
            .unwrap();
        assert_eq!(always_truthy.start, if_test);
        assert_eq!(always_truthy.end, if_test + "definitely".len());

        let fix = diagnostics
            .iter()
            .find(|d| d.message == message("neverOptionalChain"))
            .and_then(|d| d.fix.as_ref())
            .expect("optional chain fix");
        assert_eq!(fix.text, ".");
        let fix_source_start = source.find("?.").unwrap();
        assert_eq!(fix.start, fix_source_start);
        assert_eq!(fix.end, fix_source_start + 2);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Reactive exclusions: a condition over a mutable (`let`) root in a
    /// template is skipped because Svelte reactivity makes its type
    /// flow-sensitive.
    #[test]
    fn analyze_skips_mutable_roots_in_templates() {
        if crate::tsrs::ts_locator::find_on_path().is_none() {
            eprintln!("skipping: no tsrs binary on PATH");
            return;
        }
        let dir = std::env::temp_dir().join("oxvelte-tsrs-reactive-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();

        let source = concat!(
            "<script lang=\"ts\">\n",
            "let mutable: true = true;\n",
            "const fixed = true;\n",
            "</script>\n",
            "{#if mutable}<p>a</p>{/if}\n",
            "{#if fixed}<p>b</p>{/if}\n"
        );
        let script_start = source.find("let mutable").unwrap();
        let script_end = source.find("const fixed").unwrap() + "const fixed = true;".len();
        let filename = dir.join("Component.svelte");
        let mut session = TsrsSession::create(
            &filename,
            source,
            &[(script_start, script_end)],
            &[
                TemplateExpression {
                    start: source.find("mutable}").unwrap(),
                    end: source.find("mutable}").unwrap() + 7,
                    condition: true,
                },
                TemplateExpression {
                    start: source.find("fixed}").unwrap(),
                    end: source.find("fixed}").unwrap() + 5,
                    condition: true,
                },
            ],
            &serde_json::json!({}),
        )
        .expect("session");

        let diagnostics = analyze(&mut session, &serde_json::json!({})).expect("analyze");
        assert!(
            diagnostics.iter().all(|d| d.start != source.find("mutable}").unwrap()),
            "mutable root in template must be skipped: {diagnostics:?}"
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message == message("alwaysTruthy") && d.start == source.find("fixed}").unwrap()),
            "const root still reported: {diagnostics:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
