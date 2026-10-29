//! Logical spacing contracts from niri e19df12, niri-config/src/layout.rs.
//! Integer conversion belongs at the output projection, not in KDL decoding.
use crate::config::parse::parse_kdl_config;
use crate::config::InsetValue;

fn logical(value: InsetValue) -> f64 {
    match value {
        InsetValue::Logical(value) => value,
        InsetValue::Physical(value) => panic!("niri strut became physical: {value}"),
    }
}

#[test]
fn parity_fractional_spacing_global_values_survive_decode() {
    let config = parse_kdl_config(
        r#"
layout {
    gaps 2.5
    struts { left -4.5; right 0.25; top 1.75; bottom -2.25; }
}
"#,
    )
    .expect("fractional gaps and signed fractional struts are valid KDL");

    assert_eq!(
        [
            config.layout.inner_gaps,
            config.layout.outer_gaps,
            logical(config.layout.struts.left),
            logical(config.layout.struts.right),
            logical(config.layout.struts.top),
            logical(config.layout.struts.bottom),
        ],
        [2.5, 2.5, -4.5, 0.25, 1.75, -2.25],
        "logical values must survive decoding before any output scale is known"
    );
}

#[test]
fn parity_fractional_spacing_output_and_workspace_partials_preserve_values() {
    let config = parse_kdl_config(
        r#"
layout { gaps 1.25; struts { left -0.25; right 0.75; } }
output "DISPLAY1" {
    layout { gaps 2.5; struts { left -4.5; } }
}
workspace "Code" {
    layout { gaps 3.5; struts { top -6.25; } }
}
"#,
    )
    .expect("output and workspace spacing partials are valid KDL");
    let output = config
        .output
        .iter()
        .find(|output| output.name == "DISPLAY1")
        .expect("the configured output exists")
        .layout_override
        .as_ref()
        .expect("the output has a layout partial");
    let workspace = config
        .workspace
        .iter()
        .find(|workspace| workspace.name == "Code")
        .expect("the configured workspace exists")
        .layout_override
        .as_ref()
        .expect("the workspace has a layout partial");

    assert_eq!(output.inner_gaps, Some(2.5));
    assert_eq!(output.struts.left.map(logical), Some(-4.5));
    assert_eq!(workspace.inner_gaps, Some(3.5));
    assert_eq!(workspace.struts.top.map(logical), Some(-6.25));

    let output_layout = output.apply_to(&config.layout);
    let workspace_layout = workspace.apply_to(&output_layout);
    assert_eq!(output_layout.inner_gaps, 2.5);
    assert_eq!(workspace_layout.inner_gaps, 3.5);
    assert_eq!(logical(workspace_layout.struts.left), -4.5);
    assert_eq!(logical(workspace_layout.struts.top), -6.25);
    assert_eq!(logical(workspace_layout.struts.right), 0.75);
}

#[test]
fn parity_fractional_spacing_alias_order_and_explicit_physical_zero_are_preserved() {
    let config = parse_kdl_config(
        r#"
layout {
    reserve-top 10
    struts { left -4.5; top -2.25; }
    reserve-left 0
}
output "DISPLAY1" {
    layout { reserve-top 0; }
}
"#,
    )
    .expect("logical struts and physical reserve aliases may be mixed");
    assert_eq!(config.layout.struts.left, InsetValue::Physical(0));
    assert_eq!(config.layout.struts.top, InsetValue::Logical(-2.25));
    let partial = config.output[0].layout_override.as_ref().unwrap();
    assert_eq!(partial.struts.top, Some(InsetValue::Physical(0)));
    assert_eq!(partial.struts.left, None);
    let effective = partial.apply_to(&config.layout);
    assert_eq!(effective.struts.top, InsetValue::Physical(0));
    assert_eq!(effective.struts.left, InsetValue::Physical(0));
}
