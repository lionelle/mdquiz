//! Render LaTeX math as OOXML math (OMML) for a Word document.
//!
//! Two stages, deliberately split. `math-core` turns LaTeX into `MathML` — an
//! unbounded problem, thousands of commands and macro expansion, not worth
//! owning. This module owns the second stage: `MathML` to OMML, which is a
//! *bounded* mapping over a spec-defined element set. A realistic corpus of
//! instructor math produces about twenty `MathML` elements, and that is the whole
//! surface.
//!
//! # Nothing renders best-effort
//!
//! An exam is not a place for an equation that is quietly wrong. Anything that
//! cannot be represented faithfully — LaTeX `math-core` rejects, or a construct
//! with no OMML equivalent such as a table column rule — fails the export with
//! the offending LaTeX quoted. See [`crate::Error::UnsupportedMath`].
//!
//! # Panics from the parser are contained
//!
//! A third-party parser may panic on hostile input; letting that abort the
//! process would be this crate's bug, not its dependency's. Conversion runs
//! inside [`std::panic::catch_unwind`] and a caught panic becomes the same
//! error as a rejection.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use math_core::{LatexToMathML, MathCoreConfig, MathDisplay};

use crate::{Error, Result};

/// Whether an expression sits in a line of prose or on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    /// `$…$` — set inline, with the tighter spacing that implies.
    Inline,
    /// `$$…$$` — set on its own line.
    Block,
}

impl From<Display> for MathDisplay {
    /// Map onto the converter's own display flag.
    fn from(display: Display) -> Self {
        match display {
            Display::Inline => Self::Inline,
            Display::Block => Self::Block,
        }
    }
}

/// The deepest brace nesting accepted before the input is refused.
///
/// Pathologically nested LaTeX overflows the stack *inside the parser*, and a
/// stack overflow aborts the process rather than unwinding — `catch_unwind`
/// cannot contain it. Measured: `\frac{` nested 40 deep is fatal in a debug
/// build. 32 is comfortably under that and far beyond any real exam.
const MAX_NESTING: usize = 32;

/// The deepest brace nesting in `latex`.
fn nesting_depth(latex: &str) -> usize {
    let (mut depth, mut deepest) = (0_usize, 0_usize);
    for character in latex.chars() {
        match character {
            '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// Convert `latex` to an OMML fragment.
///
/// Always returns a bare `<m:oMath>` element, ready to sit inside a `w:p`.
/// `display` selects how the math is *set* — whether a sum's limits go above
/// and below its sign or beside it — and nothing more. Centring display math
/// on its own line is `m:oMathPara`, which centres the whole paragraph, so
/// only the paragraph writer can know whether it may be applied.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] when the LaTeX is rejected, when the
/// parser panics, or when the expression uses a construct Word has no way to
/// represent.
pub fn to_omml(latex: &str, display: Display) -> Result<String> {
    if nesting_depth(latex) > MAX_NESTING {
        return Err(unsupported(
            latex,
            &format!("nested more than {MAX_NESTING} deep"),
        ));
    }
    let mathml = to_mathml(latex, display)?;
    let document =
        roxmltree::Document::parse(&mathml).map_err(|error| unsupported(latex, &error))?;
    let mut body = String::new();
    render_children(document.root_element(), latex, &mut body)?;
    Ok(format!("<m:oMath>{body}</m:oMath>"))
}

/// Run `math-core` over `latex`, containing any panic it may raise.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if the LaTeX is rejected or the parser
/// panics.
fn to_mathml(latex: &str, display: Display) -> Result<String> {
    contain_panics(latex, || {
        let converter = LatexToMathML::new(MathCoreConfig::default())
            .map_err(|error| format!("converter setup failed: {error:?}"))?;
        converter
            .convert_with_local_state(latex, display.into())
            .map(|rendered| rendered.mathml)
            .map_err(|error| error.to_string())
    })
}

/// Run `convert`, turning a panic into the same error a rejection gives.
///
/// Split out so the containment can be tested: no LaTeX is known to make
/// `math-core` panic — it returns errors — so exercising this through
/// [`to_omml`] would only ever prove the rejection path.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if `convert` fails or unwinds.
fn contain_panics(
    latex: &str,
    convert: impl FnOnce() -> std::result::Result<String, String>,
) -> Result<String> {
    // `AssertUnwindSafe` because nothing observable is shared across the
    // boundary: the closure owns its converter and returns an owned String.
    match catch_unwind(AssertUnwindSafe(convert)) {
        Ok(Ok(mathml)) => Ok(mathml),
        Ok(Err(reason)) => Err(unsupported(latex, &reason)),
        Err(_) => Err(unsupported(
            latex,
            &"the math parser panicked on this input",
        )),
    }
}

/// Build the error for math that cannot be put on paper.
fn unsupported(latex: &str, reason: &impl ToString) -> Error {
    Error::UnsupportedMath {
        latex: latex.to_owned(),
        reason: reason.to_string(),
    }
}

/// Render every child of `node` into `out`.
///
/// # Errors
///
/// Propagates the first unmappable construct.
fn render_children(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let children: Vec<roxmltree::Node<'_, '_>> = node.children().filter(is_content).collect();
    render_sequence(&children, latex, out)
}

/// Render a run of sibling nodes, giving each large operator the expression
/// that follows it.
///
/// Every caller that walks siblings must go through here: rendering them one by
/// one instead leaves a large operator with no operand, which Word draws as an
/// empty placeholder box.
///
/// # Errors
///
/// Propagates the first unmappable node.
fn render_sequence(
    children: &[roxmltree::Node<'_, '_>],
    latex: &str,
    out: &mut String,
) -> Result<()> {
    let mut index = 0;
    while let Some(child) = children.get(index) {
        // A large operator takes the expression after it as its operand. `MathML`
        // leaves that as a sibling; OMML nests it, and an n-ary left empty
        // renders as an empty placeholder box on the page.
        if is_nary(*child) {
            let operand = children.get(index + 1).copied();
            render_limits_with_operand(*child, operand, latex, out)?;
            index += if operand.is_some() { 2 } else { 1 };
            continue;
        }
        render(*child, latex, out)?;
        index += 1;
    }
    Ok(())
}

/// Whether `node` is a large operator carrying limits, e.g. `\sum_{i=1}^{n}`.
///
/// Both script shapes count: `\sum` arrives as `munder*` and `\int` as
/// `msub*`, and both should become one n-ary object rather than an operator
/// with scripts stuck on — which would leave the integral sign short and its
/// integrand outside.
fn is_nary(node: roxmltree::Node<'_, '_>) -> bool {
    let scripted = matches!(
        node.tag_name().name(),
        "mover" | "munder" | "munderover" | "msub" | "msup" | "msubsup"
    );
    scripted
        && node.attribute("accent") != Some("true")
        && node.attribute("accentunder") != Some("true")
        && parts(node)
            .first()
            .copied()
            .and_then(nary_operator)
            .is_some()
}

/// Where a large operator's limits sit.
///
/// `\sum` sets them above and below, `\int` beside — which is how each reads
/// in LaTeX by default, and which `math-core`'s choice of `munder*` versus
/// `msub*` already encodes.
fn limit_location(node: roxmltree::Node<'_, '_>) -> &'static str {
    if node.tag_name().name().starts_with("mu") || node.tag_name().name() == "mover" {
        "undOvr"
    } else {
        "subSup"
    }
}

/// Whether a node carries content worth rendering (elements and real text).
fn is_content(node: &roxmltree::Node<'_, '_>) -> bool {
    node.is_element() || node.text().is_some_and(|text| !text.trim().is_empty())
}

/// Render one `MathML` node into `out`.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] for an element with no OMML equivalent.
fn render(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    if !node.is_element() {
        out.push_str(&run(node.text().unwrap_or_default(), false));
        return Ok(());
    }
    match node.tag_name().name() {
        "mi" | "mn" | "mtext" | "mo" => render_token(node, out),
        "mrow" if fences(node).is_some() => render_delimited(node, latex, out)?,
        "mrow" | "mstyle" | "semantics" => render_children(node, latex, out)?,
        "mfrac" => render_fraction(node, latex, out)?,
        "msup" | "msub" | "msubsup" if is_nary(node) => {
            render_limits_with_operand(node, None, latex, out)?;
        }
        "msup" | "msub" | "msubsup" => render_scripts(node, latex, out)?,
        "msqrt" | "mroot" => render_radical(node, latex, out)?,
        "mover" | "munder" | "munderover" => render_limits(node, latex, out)?,
        "mphantom" => render_phantom(node, latex, out)?,
        // `math-core` emits a bare `<mspace/>` before font-variant groups; a
        // width-less space rendered as a real one prints as a stray gap.
        "mspace" => {
            if node.attribute("width").is_some_and(|w| !w.starts_with('0')) {
                out.push_str(&run(" ", false));
            }
        }
        "mtable" => render_table(node, latex, out)?,
        other => {
            return Err(unsupported(
                latex,
                &format!("no Word equivalent for <{other}>"),
            ));
        }
    }
    Ok(())
}

/// Render a leaf token: an identifier, number, operator or text run.
///
/// A *single-letter* identifier is italic, the convention for a variable;
/// everything else is upright, which is what makes `log` read as a function
/// name rather than three multiplied variables. Multi-letter identifiers are
/// safe to leave upright because `math-core` has already folded font variants
/// into Unicode math-alphanumeric codepoints, so `\mathit{len}` arrives as
/// italic glyphs rather than as an attribute to honour.
fn render_token(node: roxmltree::Node<'_, '_>, out: &mut String) {
    let text = node.text().unwrap_or_default();
    let italic = node.tag_name().name() == "mi" && text.chars().count() == 1;
    out.push_str(&run(text, italic));
}

/// One OMML run. `italic` selects the variable style; everything else is set
/// upright, so multi-letter names are not mistaken for products.
fn run(text: &str, italic: bool) -> String {
    let style = if italic {
        String::new()
    } else {
        r#"<m:rPr><m:sty m:val="p"/></m:rPr>"#.to_owned()
    };
    format!(
        r#"<m:r>{style}<m:t xml:space="preserve">{}</m:t></m:r>"#,
        crate::export::escape_xml(text)
    )
}

/// Wrap `node`'s *children* in an OMML argument element.
///
/// # Errors
///
/// Propagates the first unmappable child.
fn argument_children(node: roxmltree::Node<'_, '_>, latex: &str) -> Result<String> {
    let mut inner = String::new();
    render_children(node, latex, &mut inner)?;
    Ok(format!("<m:e>{inner}</m:e>"))
}

/// Wrap `node` itself in an OMML argument element.
///
/// # Errors
///
/// Propagates the node if it cannot be mapped.
fn argument(node: roxmltree::Node<'_, '_>, latex: &str) -> Result<String> {
    let mut inner = String::new();
    render(node, latex, &mut inner)?;
    Ok(format!("<m:e>{inner}</m:e>"))
}

/// The element children of `node`, for the fixed-arity constructs.
fn parts<'a>(node: roxmltree::Node<'a, 'a>) -> Vec<roxmltree::Node<'a, 'a>> {
    node.children()
        .filter(roxmltree::Node::is_element)
        .collect()
}

/// The opening and closing delimiters of `node`, if it is a fenced group.
///
/// `math-core` marks a `\left(…\right)` group, and the brackets `\binom` and
/// the matrix environments put around their content, as stretchy `mo` children
/// at each end of an `mrow`.
fn fences(node: roxmltree::Node<'_, '_>) -> Option<(String, String)> {
    let children = parts(node);
    let (first, last) = (children.first()?, children.last()?);
    if children.len() < 3 || !is_fence(*first) || !is_fence(*last) {
        return None;
    }
    Some((fence_char(*first), fence_char(*last)))
}

/// Whether `node` is a stretchy delimiter.
fn is_fence(node: roxmltree::Node<'_, '_>) -> bool {
    node.tag_name().name() == "mo" && node.attribute("stretchy") != Some("false")
}

/// A delimiter's character, or the empty string for an invisible one.
///
/// `\left.` arrives as U+2063 INVISIBLE SEPARATOR; OMML spells "no delimiter
/// here" as an empty `m:begChr`/`m:endChr`, and passing the codepoint through
/// would print a box in some fonts.
fn fence_char(node: roxmltree::Node<'_, '_>) -> String {
    let text = node.text().unwrap_or_default().trim();
    if text.chars().all(|c| matches!(c, '\u{2061}'..='\u{2064}')) {
        return String::new();
    }
    text.to_owned()
}

/// Render a delimited group as OMML delimiters, which grow with their content.
///
/// Emitted as literal runs instead, a full-height fraction would get short
/// parentheses beside it — the commonest way generated equations look wrong.
///
/// # Errors
///
/// Propagates an unmappable child.
fn render_delimited(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let Some((open, close)) = fences(node) else {
        return render_children(node, latex, out);
    };
    let children = parts(node);
    let contents = children
        .get(1..children.len().saturating_sub(1))
        .unwrap_or_default();
    let mut inner = String::new();
    render_sequence(contents, latex, &mut inner)?;
    let _ = write!(
        out,
        r#"<m:d><m:dPr><m:begChr m:val="{}"/><m:endChr m:val="{}"/><m:grow/></m:dPr><m:e>{inner}</m:e></m:d>"#,
        crate::export::escape_xml(&open),
        crate::export::escape_xml(&close)
    );
    Ok(())
}

/// Render a fraction. A zero line thickness is how `\binom` arrives, and OMML
/// spells that as a bar-less fraction rather than a separate construct.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if the numerator or denominator is
/// missing, or either side cannot be mapped.
fn render_fraction(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let sides = parts(node);
    let (Some(numerator), Some(denominator)) = (sides.first(), sides.get(1)) else {
        return Err(unsupported(latex, &"a fraction needs two parts"));
    };
    let bar = node
        .attribute("linethickness")
        .is_some_and(|t| t.starts_with('0'));
    let properties = if bar {
        r#"<m:fPr><m:type m:val="noBar"/></m:fPr>"#
    } else {
        "<m:fPr/>"
    };
    let mut num = String::new();
    render(*numerator, latex, &mut num)?;
    let mut den = String::new();
    render(*denominator, latex, &mut den)?;
    let _ = write!(
        out,
        "<m:f>{properties}<m:num>{num}</m:num><m:den>{den}</m:den></m:f>"
    );
    Ok(())
}

/// Render a super/subscript.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if the element is missing a part.
fn render_scripts(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let sides = parts(node);
    let Some(base) = sides.first() else {
        return Err(unsupported(latex, &"a script needs a base"));
    };
    let base = argument(*base, latex)?;
    let script = |index: usize, tag: &str| -> Result<String> {
        let Some(part) = sides.get(index) else {
            return Err(unsupported(latex, &"a script is missing its exponent"));
        };
        let mut inner = String::new();
        render(*part, latex, &mut inner)?;
        Ok(format!("<m:{tag}>{inner}</m:{tag}>"))
    };
    let rendered = match node.tag_name().name() {
        "msup" => format!("<m:sSup>{base}{}</m:sSup>", script(1, "sup")?),
        "msub" => format!("<m:sSub>{base}{}</m:sSub>", script(1, "sub")?),
        _ => format!(
            "<m:sSubSup>{base}{}{}</m:sSubSup>",
            script(1, "sub")?,
            script(2, "sup")?
        ),
    };
    out.push_str(&rendered);
    Ok(())
}

/// Render a square or nth root.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if the radicand is missing.
fn render_radical(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let sides = parts(node);
    let Some(radicand) = sides.first() else {
        return Err(unsupported(latex, &"a root needs a radicand"));
    };
    let body = argument(*radicand, latex)?;
    let degree = match sides.get(1) {
        Some(part) => {
            let mut inner = String::new();
            render(*part, latex, &mut inner)?;
            format!("<m:deg>{inner}</m:deg>")
        }
        // A square root has no degree, and OMML wants that said explicitly.
        None => r#"<m:radPr><m:degHide m:val="1"/></m:radPr><m:deg/>"#.to_owned(),
    };
    let _ = write!(out, "<m:rad>{degree}{body}</m:rad>");
    Ok(())
}

/// Render a large operator together with the expression it applies to.
///
/// # Errors
///
/// Propagates an unmappable limit or operand.
fn render_limits_with_operand(
    node: roxmltree::Node<'_, '_>,
    operand: Option<roxmltree::Node<'_, '_>>,
    latex: &str,
    out: &mut String,
) -> Result<()> {
    let sides = parts(node);
    let Some(operator) = sides.first().copied().and_then(nary_operator) else {
        return render_limits(node, latex, out);
    };
    render_nary(node, &operator, &sides, operand, latex, out)
}

/// Render something set above and/or below a base.
///
/// Three OMML constructs share this `MathML` shape: an accent (a hat or bar over
/// a letter), a large operator carrying its limits (`\sum_{i=1}^{n}`), and a
/// plain under/over script. They are told apart by the `accent` attribute and
/// by whether the base is an n-ary operator.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a part is missing or unmappable.
fn render_limits(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let sides = parts(node);
    let Some(base) = sides.first() else {
        return Err(unsupported(latex, &"a limit needs a base"));
    };
    if node.attribute("accent") == Some("true") {
        return render_accent(node, *base, sides.get(1).copied(), latex, out);
    }
    if let Some(operator) = nary_operator(*base) {
        return render_nary(node, &operator, &sides, None, latex, out);
    }
    let name = node.tag_name().name();
    if name == "munderover" {
        // `m:limLow`/`m:limUpp` carry one limit each. Emitting one and dropping
        // the other would lose half the expression silently, which is worse on
        // a printed exam than refusing it.
        return Err(unsupported(
            latex,
            &"an over- and under-script on the same base has no Word equivalent",
        ));
    }
    let body = argument(*base, latex)?;
    let tag = if name == "munder" { "limLow" } else { "limUpp" };
    let Some(limit) = sides.get(1) else {
        return Err(unsupported(latex, &"a limit is missing its script"));
    };
    let mut inner = String::new();
    render(*limit, latex, &mut inner)?;
    let _ = write!(out, "<m:{tag}>{body}<m:lim>{inner}</m:lim></m:{tag}>");
    Ok(())
}

/// The operators that become a single OMML n-ary object, swallowing the
/// expression they apply to.
///
/// Where their limits sit is decided separately by [`limit_location`]: `\sum`
/// sets them above and below, `\int` beside.
const NARY_OPERATORS: [&str; 12] = ["∑", "∏", "∐", "∫", "∮", "⋃", "⋂", "⨁", "⨂", "⨀", "⋀", "⋁"];

/// The operator character if `node` is a large operator, else `None`.
fn nary_operator(node: roxmltree::Node<'_, '_>) -> Option<String> {
    let text = node.text()?.trim().to_owned();
    (node.tag_name().name() == "mo" && NARY_OPERATORS.contains(&text.as_str())).then_some(text)
}

/// Render `\sum_{i=1}^{n}` and friends as a single OMML n-ary object.
///
/// # Errors
///
/// Propagates an unmappable limit.
fn render_nary(
    node: roxmltree::Node<'_, '_>,
    operator: &str,
    sides: &[roxmltree::Node<'_, '_>],
    operand: Option<roxmltree::Node<'_, '_>>,
    latex: &str,
    out: &mut String,
) -> Result<()> {
    let under = !matches!(node.tag_name().name(), "mover" | "msup");
    let limit = |part: Option<&roxmltree::Node<'_, '_>>, tag: &str| -> Result<String> {
        let Some(part) = part else {
            return Ok(format!("<m:{tag}/>"));
        };
        let mut inner = String::new();
        render(*part, latex, &mut inner)?;
        Ok(format!("<m:{tag}>{inner}</m:{tag}>"))
    };
    let name = node.tag_name().name();
    let (sub, sup) = if matches!(name, "munderover" | "msubsup") {
        (limit(sides.get(1), "sub")?, limit(sides.get(2), "sup")?)
    } else if under {
        (limit(sides.get(1), "sub")?, "<m:sup/>".to_owned())
    } else {
        ("<m:sub/>".to_owned(), limit(sides.get(1), "sup")?)
    };
    // A slot left empty but not hidden is drawn by Word as a placeholder box,
    // the same defect an empty operand produced.
    let properties = format!(
        r#"<m:naryPr><m:chr m:val="{}"/><m:limLoc m:val="{}"/><m:subHide m:val="{}"/><m:supHide m:val="{}"/></m:naryPr>"#,
        crate::export::escape_xml(operator),
        limit_location(node),
        u8::from(sub.ends_with("<m:sub/>")),
        u8::from(sup.ends_with("<m:sup/>")),
    );
    let body = match operand {
        Some(operand) => argument(operand, latex)?,
        None => "<m:e/>".to_owned(),
    };
    let _ = write!(out, "<m:nary>{properties}{sub}{sup}{body}</m:nary>");
    Ok(())
}

/// Render an accent — a hat, bar, vector arrow or tilde over a letter.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if the accent mark is missing.
fn render_accent(
    node: roxmltree::Node<'_, '_>,
    base: roxmltree::Node<'_, '_>,
    mark: Option<roxmltree::Node<'_, '_>>,
    latex: &str,
    out: &mut String,
) -> Result<()> {
    let _ = node;
    let Some(mark) = mark.and_then(|node| node.text().map(str::trim).map(ToOwned::to_owned)) else {
        return Err(unsupported(latex, &"an accent is missing its mark"));
    };
    let body = argument(base, latex)?;
    let _ = write!(
        out,
        r#"<m:acc><m:accPr><m:chr m:val="{}"/></m:accPr>{body}</m:acc>"#,
        crate::export::escape_xml(&mark)
    );
    Ok(())
}

/// Render phantom content: space reserved, nothing drawn.
///
/// # Errors
///
/// Propagates an unmappable child.
fn render_phantom(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let mut inner = String::new();
    render_children(node, latex, &mut inner)?;
    let _ = write!(
        out,
        r#"<m:phant><m:phantPr><m:show m:val="0"/></m:phantPr><m:e>{inner}</m:e></m:phant>"#
    );
    Ok(())
}

/// Render a matrix or aligned block.
///
/// Column *rules* and `\hline` are refused rather than dropped. OMML has no
/// element for them at all — pandoc's mature converter silently discards them,
/// which would print an augmented matrix or a truth table with its dividing
/// lines missing and nothing to say so.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] for a ruled table, or for an unmappable
/// cell.
fn render_table(node: roxmltree::Node<'_, '_>, latex: &str, out: &mut String) -> Result<()> {
    let rows = parts(node);
    let columns = rows.iter().map(|row| parts(*row).len()).max().unwrap_or(0);
    let mut body = String::new();
    for row in &rows {
        let mut cells = String::new();
        for cell in parts(*row) {
            reject_rules(cell, latex)?;
            cells.push_str(&argument_children(cell, latex)?);
        }
        let _ = write!(body, "<m:mr>{cells}</m:mr>");
    }
    let alignment = rows
        .first()
        .map(|row| column_alignment(*row))
        .unwrap_or_default();
    let properties = format!(
        r#"<m:mPr><m:baseJc m:val="center"/><m:plcHide m:val="1"/><m:mcs>{alignment}</m:mcs></m:mPr>"#
    );
    let _ = columns;
    let _ = write!(out, "<m:m>{properties}{body}</m:m>");
    Ok(())
}

/// Refuse a cell carrying a rule, which Word cannot draw inside an equation.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] naming the LaTeX that asked for it.
fn reject_rules(cell: roxmltree::Node<'_, '_>, latex: &str) -> Result<()> {
    if cell
        .attribute("style")
        .is_some_and(|s| s.contains("border"))
    {
        return Err(unsupported(
            latex,
            &"Word cannot draw rules inside an equation, so a `|` column or \
             `\\hline` would print as an unruled grid",
        ));
    }
    Ok(())
}

/// The per-column justification, read from the first row's cells.
///
/// `MathML` carries alignment as CSS; OMML as `m:mcJc`. Only left, centre and
/// right exist on either side, so the mapping is total.
fn column_alignment(row: roxmltree::Node<'_, '_>) -> String {
    parts(row).iter().fold(String::new(), |mut out, cell| {
        let style = cell.attribute("style").unwrap_or_default();
        let justify = if style.contains("text-align: right") {
            "right"
        } else if style.contains("text-align: left") {
            "left"
        } else {
            "center"
        };
        let _ = write!(
            out,
            r#"<m:mc><m:mcPr><m:count m:val="1"/><m:mcJc m:val="{justify}"/></m:mcPr></m:mc>"#
        );
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert inline math, expecting success.
    fn omml(latex: &str) -> String {
        to_omml(latex, Display::Inline).expect("converts")
    }

    /// The rendered error from math expected to be refused.
    fn refused(latex: &str) -> String {
        to_omml(latex, Display::Inline)
            .expect_err("should be refused")
            .to_string()
    }

    #[test]
    /// The everyday complexity notation an instructor writes: an upright
    /// function name, an italic variable, and parentheses that do not stretch.
    fn renders_complexity_notation() {
        let xml = omml(r"O(n \log n)");
        assert!(xml.starts_with("<m:oMath>") && xml.ends_with("</m:oMath>"));
        assert!(
            xml.contains("<m:t xml:space=\"preserve\">log</m:t>"),
            "{xml}"
        );
        // `log` is upright, `n` is italic: a multi-letter name must not read as
        // a product of variables.
        assert!(
            xml.contains(r#"<m:sty m:val="p"/></m:rPr><m:t xml:space="preserve">log</m:t>"#),
            "{xml}"
        );
        assert!(xml.contains("<m:t xml:space=\"preserve\">n</m:t>"));
    }

    #[test]
    /// Superscripts, subscripts and both together each reach their own OMML
    /// construct.
    fn renders_scripts() {
        assert!(omml("n^2").contains("<m:sSup>"));
        assert!(omml("a_i").contains("<m:sSub>"));
        assert!(omml("x_i^2").contains("<m:sSubSup>"));
    }

    #[test]
    /// A fraction is a fraction; a binomial is the same construct with the bar
    /// switched off, which is how `math-core` expresses it.
    fn renders_fractions_and_binomials() {
        let frac = omml(r"\frac{a}{b}");
        assert!(frac.contains("<m:f>") && frac.contains("<m:num>") && frac.contains("<m:den>"));
        assert!(!frac.contains("noBar"));
        assert!(omml(r"\binom{n}{k}").contains(r#"<m:type m:val="noBar"/>"#));
    }

    #[test]
    /// A square root hides its degree; an nth root states it.
    fn renders_roots() {
        let sqrt = omml(r"\sqrt{2}");
        assert!(sqrt.contains("<m:rad>") && sqrt.contains("degHide"));
        let cube = omml(r"\sqrt[3]{x}");
        assert!(cube.contains("<m:deg>") && !cube.contains("degHide"));
    }

    #[test]
    /// A summation becomes one n-ary object carrying its own limits, rather
    /// than an operator with scripts stuck on.
    fn renders_summation_as_an_nary() {
        let xml = omml(r"\sum_{i=1}^{n} i");
        assert!(xml.contains("<m:nary>"), "{xml}");
        assert!(xml.contains(r#"<m:chr m:val="∑"/>"#), "{xml}");
        assert!(xml.contains("<m:sub>") && xml.contains("<m:sup>"));
    }

    #[test]
    /// A large operator swallows the expression it applies to. `MathML` leaves
    /// that expression as a sibling; OMML nests it, and an n-ary left empty
    /// prints an empty placeholder box on the page.
    fn a_large_operator_nests_the_expression_after_it() {
        let xml = omml(r"\sum_{i=1}^{n} i");
        assert!(!xml.contains("<m:e/>"), "empty operand placeholder: {xml}");
        assert!(
            !xml.contains("<m:e></m:e>"),
            "empty operand placeholder: {xml}"
        );
        // The `i` is inside the n-ary, not trailing after it.
        let nary = xml.split("<m:nary>").nth(1).unwrap_or_default();
        let body = nary.split("</m:nary>").next().unwrap_or_default();
        assert!(
            body.contains(r#"<m:t xml:space="preserve">i</m:t>"#),
            "{xml}"
        );
    }

    /// The content of the first `<m:tag>…</m:tag>` — for asking what landed
    /// *inside* a construct, not merely that the construct exists.
    fn inside<'a>(xml: &'a str, tag: &str) -> &'a str {
        xml.split_once(&format!("<m:{tag}>"))
            .and_then(|(_, rest)| rest.split_once(&format!("</m:{tag}>")))
            .map_or("", |(inner, _)| inner)
    }

    #[test]
    /// Pathologically nested input is refused before the parser sees it. A
    /// stack overflow inside the parser is an abort, not an unwind:
    /// `catch_unwind` cannot contain it and it takes the whole export down.
    fn absurdly_nested_latex_is_refused_rather_than_crashing() {
        let latex = format!("{}x{}", r"\frac{".repeat(64), "}{1}".repeat(64));
        let error = refused(&latex);
        assert!(error.contains("nested more than"), "{error}");
        // A realistic depth is untouched.
        assert!(to_omml(r"\frac{\frac{a}{b}}{c}", Display::Inline).is_ok());
    }

    #[test]
    /// A panic in the converter becomes an error rather than escaping. Proved
    /// against a converter that really unwinds: no LaTeX is known to panic
    /// `math-core`, so testing this through `to_omml` would only ever exercise
    /// the rejection path.
    fn a_panicking_converter_becomes_an_error() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = contain_panics(r"\x", || {
            // Panics with "removal index out of bounds" — a genuine unwind,
            // written without `expect`/`unwrap`/indexing, all of which the
            // crate denies even in tests.
            Vec::<String>::new().remove(0);
            Ok(String::new())
        });
        std::panic::set_hook(hook);
        let error = result.expect_err("a panic must not escape").to_string();
        assert!(error.contains("panicked on this input"), "{error}");
        assert!(error.contains(r"\x"), "{error}");
    }

    #[test]
    /// A one-sided operator hides the slot it has no limit for. An empty
    /// `<m:sub/>` left visible prints the same placeholder box a missing
    /// operand did.
    fn a_one_sided_operator_hides_its_empty_slot() {
        let above = omml(r"\sum^{n} x");
        assert!(above.contains(r#"<m:subHide m:val="1"/>"#), "{above}");
        let below = omml(r"\bigcup_{i} A");
        assert!(below.contains(r#"<m:supHide m:val="1"/>"#), "{below}");
        // Both present means neither is hidden.
        let both = omml(r"\sum_{i=1}^{n} i");
        assert!(both.contains(r#"<m:subHide m:val="0"/>"#), "{both}");
        assert!(both.contains(r#"<m:supHide m:val="0"/>"#), "{both}");
    }

    #[test]
    /// An integral becomes one n-ary object with its limits beside it, not an
    /// operator with scripts stuck on — otherwise the sign does not grow.
    fn an_integral_is_a_large_operator_too() {
        let xml = omml(r"\int_0^1 f");
        assert!(xml.contains("<m:nary>"), "{xml}");
        assert!(xml.contains(r#"<m:chr m:val="∫"/>"#), "{xml}");
        assert!(xml.contains(r#"<m:limLoc m:val="subSup"/>"#), "{xml}");
        // A summation still sets its limits above and below.
        assert!(omml(r"\sum_{i=1}^{n} i").contains(r#"<m:limLoc m:val="undOvr"/>"#));
    }

    #[test]
    /// An over/under pair Word cannot express is refused rather than having
    /// one of its two scripts silently dropped. `\lim_{a}^{b}` is a
    /// `munderover` whose base is not a large operator, so it has no n-ary
    /// form to fall back on.
    fn an_unmappable_over_under_pair_is_refused() {
        let error = refused(r"\lim_{a}^{b} x");
        assert!(error.contains("no Word equivalent"), "{error}");
    }

    #[test]
    /// A zero-width space is not a space. `math-core` emits a bare `<mspace/>`
    /// before a font-variant group; rendering it prints a stray gap.
    fn a_zero_width_space_prints_nothing() {
        let xml = omml(r"\mathrm{len}");
        assert!(
            !xml.contains(r#"<m:t xml:space="preserve"> </m:t>"#),
            "a stray space reached the page: {xml}"
        );
    }

    #[test]
    /// A thin fraction rule is still a rule. Only an explicit zero means the
    /// bar-less form that `\binom` uses.
    fn a_thin_rule_is_not_a_missing_one() {
        assert!(!omml(r"\frac{a}{b}").contains("noBar"));
        assert!(omml(r"\binom{n}{k}").contains("noBar"));
    }

    #[test]
    /// A single-letter variable is italic and a multi-letter function name is
    /// not — `contains(">n<")` alone matches the upright rendering too.
    fn a_variable_is_italic_and_a_function_name_is_not() {
        let xml = omml(r"O(n \log n)");
        assert!(
            xml.contains(r#"<m:r><m:t xml:space="preserve">n</m:t></m:r>"#),
            "n lost its italic: {xml}"
        );
        assert!(
            !xml.contains(r#"<m:sty m:val="p"/></m:rPr><m:t xml:space="preserve">n</m:t>"#),
            "n is upright: {xml}"
        );
    }

    #[test]
    /// Fixed-arity parts do not swap: the numerator is on top, the subscript
    /// below, the degree outside the radical.
    fn fixed_arity_parts_do_not_swap() {
        let frac = omml(r"\frac{a}{b}");
        assert!(inside(&frac, "num").contains(">a<"), "{frac}");
        assert!(inside(&frac, "den").contains(">b<"), "{frac}");
        let script = omml("x_i^2");
        assert!(inside(&script, "sub").contains(">i<"), "{script}");
        assert!(inside(&script, "sup").contains(">2<"), "{script}");
        let root = omml(r"\sqrt[3]{x}");
        assert!(inside(&root, "deg").contains(">3<"), "{root}");
    }

    #[test]
    /// Delimiters grow with what they hold. Emitted as plain runs, a
    /// full-height fraction gets short parentheses beside it — the commonest
    /// way a generated equation looks wrong.
    fn delimiters_grow_with_their_content() {
        let binom = omml(r"\binom{n}{k}");
        assert!(binom.contains("<m:d>"), "{binom}");
        assert!(binom.contains(r#"<m:begChr m:val="("/>"#), "{binom}");
        assert!(binom.contains("<m:grow/>"), "{binom}");
        assert!(
            to_omml(r"\left[ x \right]", Display::Inline)
                .expect("converts")
                .contains(r#"<m:begChr m:val="["/>"#)
        );
    }

    #[test]
    /// A large operator keeps its operand wherever it sits. Delimited groups
    /// walk their own children, so the look-ahead has to be shared or a
    /// bracketed sum loses its operand and prints a placeholder box.
    fn a_large_operator_keeps_its_operand_inside_delimiters() {
        let xml = omml(r"\left[ \sum_{i=1}^{n} x \right]");
        assert!(!xml.contains("<m:e/>"), "placeholder box: {xml}");
        let nary = inside(&xml, "nary");
        assert!(nary.contains(">x<"), "operand escaped the sigma: {xml}");
    }

    #[test]
    /// An accent is an accent, not a script.
    fn renders_accents() {
        let xml = omml(r"\hat{x}");
        assert!(xml.contains("<m:acc>"), "{xml}");
        assert!(xml.contains("<m:chr m:val="), "{xml}");
    }

    #[test]
    /// A matrix becomes an OMML matrix, one row per row.
    fn renders_matrices() {
        let xml = omml(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}");
        assert!(xml.contains("<m:m>"), "{xml}");
        assert_eq!(xml.matches("<m:mr>").count(), 2, "{xml}");
    }

    #[test]
    /// Display math is *not* wrapped in `m:oMathPara` here. That element
    /// centres the paragraph around it, so applying it is the paragraph
    /// writer's call — it is the only one that knows whether a question
    /// number shares the line.
    fn display_math_is_not_set_apart_here() {
        let xml = to_omml(r"\frac{a}{b}", Display::Block).expect("converts");
        assert!(xml.starts_with("<m:oMath>"), "{xml}");
        assert!(!xml.contains("oMathPara"), "{xml}");
    }

    #[test]
    /// Text is XML-escaped, so an authored `<` cannot corrupt the document.
    fn authored_text_is_escaped() {
        let xml = omml("a < b");
        assert!(xml.contains("&lt;"), "{xml}");
        assert!(!xml.contains("<m:t xml:space=\"preserve\"><"), "{xml}");
    }

    #[test]
    /// LaTeX the converter rejects fails the export, quoting what the author
    /// wrote rather than rendering something else.
    fn rejected_latex_is_refused_with_its_source() {
        let error = refused(r"\ce{H2O}");
        assert!(error.contains(r"\ce{H2O}"), "{error}");
        assert!(error.contains("cannot render math"), "{error}");
    }

    #[test]
    /// A `|` column or `\hline` is refused. Word cannot draw rules inside an
    /// equation, and printing an augmented matrix without its bar — which is
    /// what every other converter does — is worse than failing.
    fn ruled_tables_are_refused_rather_than_flattened() {
        let error = refused(r"\begin{array}{c|r} a & b \\ c & d \end{array}");
        assert!(error.contains("rules inside an equation"), "{error}");
        // An unruled array of the same shape is fine.
        assert!(omml(r"\begin{array}{cr} a & b \\ c & d \end{array}").contains("<m:m>"));
    }

    #[test]
    /// Column alignment survives: `MathML` carries it as CSS, OMML as `m:mcJc`.
    fn column_alignment_is_carried_across() {
        let xml = omml(r"\begin{array}{lr} a & b \end{array}");
        assert!(xml.contains(r#"<m:mcJc m:val="left"/>"#), "{xml}");
        assert!(xml.contains(r#"<m:mcJc m:val="right"/>"#), "{xml}");
    }

    #[test]
    /// Hostile input is contained: the export fails, the process does not.
    /// A dependency is allowed to panic; letting it escape would be our bug.
    fn a_parser_panic_becomes_an_error() {
        for latex in ["\\char é", "\\symbol é", "\\char \u{0}"] {
            let result = to_omml(latex, Display::Inline);
            assert!(result.is_err(), "{latex:?} should not have rendered");
        }
    }
}
