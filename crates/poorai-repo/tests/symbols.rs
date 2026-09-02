//! Symbol extraction across languages.
//!
//! A symbol definition outranks a path match five to one in retrieval, so a
//! language whose declarations are invisible loses the strongest ranking
//! signal precisely where the agent knows the code least.

use poorai_repo::extract_symbols;

fn finds(source: &str, expected: &[&str]) {
    let symbols = extract_symbols(source);
    for name in expected {
        assert!(
            symbols.contains(&name.to_string()),
            "{name} not found in {symbols:?}"
        );
    }
}

#[test]
fn rust_declarations() {
    finds(
        "pub fn shipping_cost(g: i32) -> i32 { 0 }\nstruct Cart;\nenum Status { A }\ntrait Pricing {}\nimpl Cart {}\n",
        &["shipping_cost", "Cart", "Status", "Pricing"],
    );
}

#[test]
fn python_declarations() {
    finds(
        "def calculate_total(items):\n    pass\n\nclass ShoppingCart:\n    def add_item(self): pass\n",
        &["calculate_total", "ShoppingCart", "add_item"],
    );
}

#[test]
fn javascript_and_typescript_declarations() {
    finds(
        "export function applyTariff(a, b) {}\nclass OrderService {}\ninterface Invoice {}\ntype Money = number;\n",
        &["applyTariff", "OrderService", "Invoice", "Money"],
    );
}

#[test]
fn go_declarations() {
    finds(
        "func ShippingCost(g int) int { return 0 }\ntype Cart struct {}\npackage pricing\n",
        &["ShippingCost", "Cart", "pricing"],
    );
}

#[test]
fn java_and_csharp_declarations() {
    finds(
        "public class InvoiceService {}\nprivate static final class Helper {}\npublic interface Repository {}\nnamespace Billing {}\npublic record Money(int cents);\n",
        &["InvoiceService", "Helper", "Repository", "Billing", "Money"],
    );
}

#[test]
fn swift_and_dart_declarations() {
    finds(
        "protocol Payable {}\nextension Cart {}\nactor Bank {}\nclass Widget extends StatelessWidget {}\nmixin Logging {}\n",
        &["Payable", "Cart", "Bank", "Widget", "Logging"],
    );
}

/// A comment describing a function is not a declaration of it, and a
/// control-flow line is not either.
#[test]
fn prose_and_control_flow_are_not_symbols() {
    let symbols = extract_symbols(
        "// fn commented_out(x: i32)\n# def also_commented():\n * class InADocComment\nif condition {\n    return value;\n}\n",
    );
    assert!(
        symbols.is_empty(),
        "extracted from comments or control flow: {symbols:?}"
    );
}

/// The regression that motivated this: Python contributed nothing.
#[test]
fn a_python_file_is_no_longer_invisible() {
    assert!(!extract_symbols("def parse_port(text):\n    return None\n").is_empty());
}
