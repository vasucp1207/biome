use biome_analyze::{
    context::RuleContext, declare_rule, ActionCategory, Ast, FixKind, Rule, RuleDiagnostic,
};
use biome_console::markup;
use biome_diagnostics::Applicability;
use biome_js_factory::make::{self, jsx_string_literal};
use biome_js_syntax::{
    AnyJsLiteralExpression, JsCallExpression, JsImportCallExpression, JsModuleSource,
};
use biome_rowan::{declare_node_union, AstNode, AstSeparatedList, BatchMutationExt};

use crate::{globals::node::NODE_BUILTINS, JsRuleAction};

declare_node_union! {
    pub(crate) AnyJsImportLike = JsModuleSource | JsCallExpression |  JsImportCallExpression
}

declare_rule! {
    /// Enforces using the `node:` protocol for Node.js builtin modules.
    ///
    /// The rule marks traditional imports like `import fs from "fs";` as invalid,
    /// suggesting the format `import fs from "node:fs";` instead.
    ///
    /// Source: https://github.com/sindresorhus/eslint-plugin-unicorn/blob/main/docs/rules/prefer-node-protocol.md
    ///
    /// ## Examples
    ///
    /// ### Invalid
    ///
    /// ```js,expect_diagnostic
    /// import fs from 'fs';
    /// ```
    ///
    /// ```js,expect_diagnostic
    /// import os from 'os';
    /// ```
    ///
    /// ```js,expect_diagnostic
    /// import path from 'path';
    /// ```
    ///
    /// ### Valid
    ///
    /// ```js
    /// import fs from 'node:fs';
    ///
    /// import os from 'node:os';
    ///
    /// import path from 'node:path';
    /// ```
    ///
    pub(crate) UseNodeImportProtocol {
        version: "next",
        name: "useNodeImportProtocol",
        recommended: false,
        fix_kind: FixKind::Unsafe,
    }
}

impl Rule for UseNodeImportProtocol {
    type Query = Ast<AnyJsImportLike>;
    type State = String;
    type Signals = Option<Self::State>;
    type Options = ();

    fn run(ctx: &RuleContext<Self>) -> Self::Signals {
        let binding = ctx.query();
        match binding {
            AnyJsImportLike::JsModuleSource(source) => {
                let source = source.inner_string_text().ok()?;
                if is_node_module_without_protocol(source.text()) {
                    return Some(source.to_string());
                }
            }
            AnyJsImportLike::JsCallExpression(expression) => {
                let callee = expression.callee().ok()?;
                let callee = callee.as_js_identifier_expression()?;
                let name = callee.name().ok()?.name().ok()?;
                if name.text() == "require" || name.text() == "import" {
                    let arguments = expression.arguments().ok()?.args();
                    if arguments.len() == 1 {
                        // SAFETY: the list has one argument, checked by the if before
                        let argument = arguments.iter().next().unwrap().ok()?;
                        let argument = argument
                            .as_any_js_expression()?
                            .as_any_js_literal_expression()?
                            .as_js_string_literal_expression()?
                            .inner_string_text()
                            .ok()?;
                        if is_node_module_without_protocol(argument.text()) {
                            return Some(argument.to_string());
                        }
                    }
                }
            }
            AnyJsImportLike::JsImportCallExpression(import_call) => {
                let arguments = import_call.arguments().ok()?.args();
                if arguments.len() == 1 {
                    // SAFETY: the list has one argument, checked by the if before
                    let argument = arguments.iter().next().unwrap().ok()?;
                    let argument = argument
                        .as_any_js_expression()?
                        .as_any_js_literal_expression()?
                        .as_js_string_literal_expression()?
                        .inner_string_text()
                        .ok()?;
                    if is_node_module_without_protocol(argument.text()) {
                        return Some(argument.to_string());
                    }
                }
            }
        };

        None
    }

    fn diagnostic(ctx: &RuleContext<Self>, state: &Self::State) -> Option<RuleDiagnostic> {
        let binding = ctx.query();

        Some(RuleDiagnostic::new(
            rule_category!(),
            binding.range(),
            markup! {
                "Import from Node.js builtin module \""<Emphasis>{state}</Emphasis>"\" should use the \""<Emphasis>"node:"</Emphasis>"\" protocol."
            },
        )
        .note(markup!{
            "Using the "<Emphasis>"node:"</Emphasis>" protocol is more explicit and signals that the imported module belongs to Node.js."
        }))
    }

    fn action(ctx: &RuleContext<Self>, state: &Self::State) -> Option<JsRuleAction> {
        let new_import_str = format!("node:{}", state);
        let old_node = biome_js_syntax::AnyJsExpression::AnyJsLiteralExpression(
            AnyJsLiteralExpression::JsStringLiteralExpression(make::js_string_literal_expression(
                jsx_string_literal(state.as_str()),
            )),
        );
        let new_node = biome_js_syntax::AnyJsExpression::AnyJsLiteralExpression(
            AnyJsLiteralExpression::JsStringLiteralExpression(make::js_string_literal_expression(
                jsx_string_literal(new_import_str.as_str()),
            )),
        );

        let mut mutation = ctx.root().begin();
        mutation.replace_node(old_node, new_node);

        Some(JsRuleAction {
            category: ActionCategory::QuickFix,
            applicability: Applicability::MaybeIncorrect,
            message: markup! { "Change to \"node:"{state}"\"." }.to_owned(),
            mutation,
        })
    }
}

fn is_node_module_without_protocol(module_name: &str) -> bool {
    !module_name.starts_with("node:") && NODE_BUILTINS.binary_search(&module_name).is_ok()
}
