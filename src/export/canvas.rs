//! Render an item bank to a Canvas *New Quizzes* QTI package.
//!
//! Canvas imports item banks as a zipped IMS content package: an
//! `imsmanifest.xml` plus one QTI 1.2 assessment document. This is the same
//! flavor Canvas's own quiz export produces, which New Quizzes migrates on
//! import. The byte payload returned here is written to a `.zip` file and
//! imported through Canvas's "QTI .zip file" option.
//!
//! XML is assembled from the module-level templates below by placeholder
//! substitution; all authored text is escaped (prompts are first rendered from
//! Markdown to HTML, then XML-escaped for embedding).

use std::io::{Cursor, Write};

use pulldown_cmark::{Parser, html};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::Result;
use crate::model::{Feedback, ItemBank, Question, QuestionKind, TrueFalse};

/// The `response_label` ident for the "True" choice; fills the item template.
const TRUE_CHOICE_IDENT: &str = "true_choice";
/// The `response_label` ident for the "False" choice; fills the item template.
const FALSE_CHOICE_IDENT: &str = "false_choice";
/// The `<itemfeedback>` ident for the always-shown general feedback.
const GENERAL_FB_IDENT: &str = "general_fb";
/// The `<itemfeedback>` ident for correct-answer feedback.
const CORRECT_FB_IDENT: &str = "correct_fb";
/// The `<itemfeedback>` ident for incorrect-answer feedback.
const INCORRECT_FB_IDENT: &str = "incorrect_fb";

/// The IMS content-package manifest referencing the single assessment resource.
const MANIFEST_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<manifest identifier="man_{{RESOURCE_IDENT}}" xmlns="http://www.imsglobal.org/xsd/imscp_v1p1" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.imsglobal.org/xsd/imscp_v1p1 http://www.imsglobal.org/xsd/imscp_v1p1.xsd">
  <metadata>
    <schema>IMS Content</schema>
    <schemaversion>1.1.3</schemaversion>
  </metadata>
  <organizations/>
  <resources>
    <resource identifier="{{RESOURCE_IDENT}}" type="imsqti_xmlv1p2/imscc_xmlv1p1/assessment" href="{{HREF}}">
      <file href="{{HREF}}"/>
    </resource>
  </resources>
</manifest>
"#;

/// The QTI 1.2 assessment wrapper; `{{ITEMS}}` holds the rendered items.
const ASSESSMENT_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<questestinterop xmlns="http://www.imsglobal.org/xsd/ims_qtiasiv1p2" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.imsglobal.org/xsd/ims_qtiasiv1p2 http://www.imsglobal.org/xsd/ims_qtiasiv1p2p1.xsd">
  <assessment ident="{{IDENT}}" title="{{TITLE}}">
    <section ident="root_section">
{{ITEMS}}    </section>
  </assessment>
</questestinterop>
"#;

/// A single true/false QTI item, scored 100 on the correct choice.
const TRUE_FALSE_ITEM_TEMPLATE: &str = r#"      <item ident="{{ITEM_IDENT}}" title="{{ITEM_TITLE}}">
        <itemmetadata>
          <qtimetadata>
            <qtimetadatafield>
              <fieldlabel>question_type</fieldlabel>
              <fieldentry>true_false_question</fieldentry>
            </qtimetadatafield>
            <qtimetadatafield>
              <fieldlabel>points_possible</fieldlabel>
              <fieldentry>{{POINTS}}</fieldentry>
            </qtimetadatafield>
          </qtimetadata>
        </itemmetadata>
        <presentation>
          <material>
            <mattext texttype="text/html">{{PROMPT}}</mattext>
          </material>
          <response_lid ident="response1" rcardinality="Single">
            <render_choice>
              <response_label ident="{{TRUE_IDENT}}">
                <material><mattext texttype="text/plain">True</mattext></material>
              </response_label>
              <response_label ident="{{FALSE_IDENT}}">
                <material><mattext texttype="text/plain">False</mattext></material>
              </response_label>
            </render_choice>
          </response_lid>
        </presentation>
{{RESPROCESSING}}{{ITEMFEEDBACK}}      </item>
"#;

/// The resprocessing wrapper; `{{CONDITIONS}}` holds the scored/feedback arms.
const RESPROCESSING_TEMPLATE: &str = r#"        <resprocessing>
          <outcomes>
            <decvar maxvalue="100" minvalue="0" varname="SCORE" vartype="Decimal"/>
          </outcomes>
{{CONDITIONS}}        </resprocessing>
"#;

/// The always-true condition that shows general feedback and keeps processing;
/// `{{FEEDBACK}}` is a display-link line built from the ident const.
const GENERAL_CONDITION: &str = r#"          <respcondition continue="Yes">
            <conditionvar>
              <other/>
            </conditionvar>
{{FEEDBACK}}          </respcondition>
"#;

/// The scoring condition; `{{CORRECT_FEEDBACK}}` is a display line or empty.
const SCORING_CONDITION: &str = r#"          <respcondition continue="No">
            <conditionvar>
              <varequal respident="response1">{{CORRECT}}</varequal>
            </conditionvar>
            <setvar action="Set" varname="SCORE">100</setvar>
{{CORRECT_FEEDBACK}}          </respcondition>
"#;

/// The condition that shows incorrect feedback on the wrong choice;
/// `{{FEEDBACK}}` is a display-link line built from the ident const.
const INCORRECT_CONDITION: &str = r#"          <respcondition continue="No">
            <conditionvar>
              <varequal respident="response1">{{INCORRECT}}</varequal>
            </conditionvar>
{{FEEDBACK}}          </respcondition>
"#;

/// A single feedback display link; `{{IDENT}}` names the `<itemfeedback>` it
/// points at, so every link is derived from the same ident const as its block.
const DISPLAY_FEEDBACK_TEMPLATE: &str =
    "            <displayfeedback feedbacktype=\"Response\" linkrefid=\"{{IDENT}}\"/>\n";

/// One `<itemfeedback>` block holding an HTML feedback message.
const ITEM_FEEDBACK_TEMPLATE: &str = r#"      <itemfeedback ident="{{IDENT}}">
        <flow_mat>
          <material>
            <mattext texttype="text/html">{{HTML}}</mattext>
          </material>
        </flow_mat>
      </itemfeedback>
"#;

/// Render `bank` as the bytes of a Canvas New Quizzes QTI package.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] if the bank contains an unsupported
/// question type or the zip package cannot be built.
pub fn to_qti(bank: &ItemBank) -> Result<Vec<u8>> {
    let assessment_ident = format!("assessment_{}", sanitize_ident(&bank.name));
    let href = format!("{assessment_ident}/{assessment_ident}.xml");
    let assessment = assessment_xml(&assessment_ident, bank)?;
    let manifest = manifest_xml(&assessment_ident, &href);
    zip_package(&[
        ("imsmanifest.xml", manifest.as_str()),
        (href.as_str(), assessment.as_str()),
    ])
}

/// Fill the manifest template with the assessment resource ident and href.
fn manifest_xml(resource_ident: &str, href: &str) -> String {
    MANIFEST_TEMPLATE
        .replace("{{RESOURCE_IDENT}}", &escape_xml(resource_ident))
        .replace("{{HREF}}", &escape_xml(href))
}

/// Render the assessment document, one item per question.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] for a question type not yet supported.
fn assessment_xml(ident: &str, bank: &ItemBank) -> Result<String> {
    let mut items = String::new();
    for question in &bank.items {
        items.push_str(&item_xml(question)?);
    }
    Ok(ASSESSMENT_TEMPLATE
        .replace("{{IDENT}}", &escape_xml(ident))
        .replace("{{TITLE}}", &escape_xml(&bank.name))
        // Replace `{{ITEMS}}` last so item text cannot collide with a template.
        .replace("{{ITEMS}}", &items))
}

/// Render one question to its QTI item XML, dispatching on kind.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] for a question type not yet supported.
fn item_xml(question: &Question) -> Result<String> {
    match &question.kind {
        QuestionKind::TrueFalse(answer) => Ok(true_false_item_xml(question, answer)),
        other => Err(other.unsupported_by("Canvas", &question.id)),
    }
}

/// Render a true/false item, marking the correct choice and any feedback.
fn true_false_item_xml(question: &Question, answer: &TrueFalse) -> String {
    let (correct, incorrect) = if answer.answer {
        (TRUE_CHOICE_IDENT, FALSE_CHOICE_IDENT)
    } else {
        (FALSE_CHOICE_IDENT, TRUE_CHOICE_IDENT)
    };
    let title = question.title.as_deref().unwrap_or(&question.id);
    let feedback = &question.feedback;
    // `{{PROMPT}}` is substituted last so authored text is never re-scanned.
    TRUE_FALSE_ITEM_TEMPLATE
        .replace("{{TRUE_IDENT}}", TRUE_CHOICE_IDENT)
        .replace("{{FALSE_IDENT}}", FALSE_CHOICE_IDENT)
        .replace("{{ITEM_IDENT}}", &sanitize_ident(&question.id))
        .replace("{{ITEM_TITLE}}", &escape_xml(title))
        .replace("{{POINTS}}", &question.points.to_string())
        .replace(
            "{{RESPROCESSING}}",
            &resprocessing_xml(correct, incorrect, feedback),
        )
        .replace("{{ITEMFEEDBACK}}", &itemfeedback_xml(feedback))
        .replace("{{PROMPT}}", &escaped_html(&question.prompt))
}

/// Build the resprocessing block: scoring plus any feedback display arms.
fn resprocessing_xml(correct: &str, incorrect: &str, feedback: &Feedback) -> String {
    let mut conditions = String::new();
    if feedback.general.is_some() {
        conditions.push_str(
            &GENERAL_CONDITION.replace("{{FEEDBACK}}", &display_feedback(GENERAL_FB_IDENT)),
        );
    }
    let correct_feedback = if feedback.correct.is_some() {
        display_feedback(CORRECT_FB_IDENT)
    } else {
        String::new()
    };
    conditions.push_str(
        &SCORING_CONDITION
            .replace("{{CORRECT}}", correct)
            .replace("{{CORRECT_FEEDBACK}}", &correct_feedback),
    );
    if feedback.incorrect.is_some() {
        conditions.push_str(
            &INCORRECT_CONDITION
                .replace("{{INCORRECT}}", incorrect)
                .replace("{{FEEDBACK}}", &display_feedback(INCORRECT_FB_IDENT)),
        );
    }
    RESPROCESSING_TEMPLATE.replace("{{CONDITIONS}}", &conditions)
}

/// Build one `<displayfeedback>` link line pointing at `ident`.
fn display_feedback(ident: &str) -> String {
    DISPLAY_FEEDBACK_TEMPLATE.replace("{{IDENT}}", ident)
}

/// Render authored Markdown to HTML and XML-escape it for a `mattext` body.
fn escaped_html(markdown: &str) -> String {
    escape_xml(&prompt_html(markdown))
}

/// Build the `<itemfeedback>` blocks for whichever feedback entries are set.
fn itemfeedback_xml(feedback: &Feedback) -> String {
    let entries = [
        (GENERAL_FB_IDENT, feedback.general.as_deref()),
        (CORRECT_FB_IDENT, feedback.correct.as_deref()),
        (INCORRECT_FB_IDENT, feedback.incorrect.as_deref()),
    ];
    let mut out = String::new();
    for (ident, text) in entries {
        if let Some(text) = text {
            out.push_str(
                &ITEM_FEEDBACK_TEMPLATE
                    .replace("{{IDENT}}", ident)
                    .replace("{{HTML}}", &escaped_html(text)),
            );
        }
    }
    out
}

/// Render a Markdown prompt to HTML for embedding in a `text/html` mattext.
fn prompt_html(markdown: &str) -> String {
    let mut rendered = String::new();
    html::push_html(&mut rendered, Parser::new(markdown));
    rendered.trim().to_owned()
}

/// XML-escape the five predefined entities, `&` first to avoid double-escaping.
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Reduce an authored string to a safe QTI identifier / path component.
fn sanitize_ident(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "id".to_owned()
    } else {
        cleaned
    }
}

/// Zip the `(path, content)` files into a package byte payload.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] if the zip archive cannot be written.
fn zip_package(files: &[(&str, &str)]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default();
        for (path, content) in files {
            writer.start_file(*path, options).map_err(|e| zip_err(&e))?;
            writer.write_all(content.as_bytes())?;
        }
        writer.finish().map_err(|e| zip_err(&e))?;
    }
    Ok(cursor.into_inner())
}

/// Map a zip-writer failure onto the crate export error.
fn zip_err(error: &zip::result::ZipError) -> crate::Error {
    crate::Error::Export(format!("building QTI zip package: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a one-question true/false bank with the given answer and prompt.
    fn true_false_bank(answer: bool, prompt: &str) -> ItemBank {
        ItemBank {
            name: "Module 1".to_owned(),
            items: vec![Question {
                id: "tf-1".to_owned(),
                title: None,
                prompt: prompt.to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer }),
            }],
        }
    }

    #[test]
    /// The package is a zip holding the manifest and one assessment file.
    fn package_is_zip_with_manifest_and_assessment() {
        let bytes = to_qti(&true_false_bank(true, "Q?")).expect("export succeeds");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        let names: Vec<&str> = archive.file_names().collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"imsmanifest.xml"));
        assert!(names.iter().any(|name| *name != "imsmanifest.xml"));
    }

    #[test]
    /// A `true` answer marks the true choice as the 100-point response.
    fn true_answer_marks_true_choice_correct() {
        let xml = assessment_xml("a", &true_false_bank(true, "Binary search needs sorting."))
            .expect("assessment renders");
        assert!(xml.contains("true_false_question"));
        assert!(xml.contains("Binary search needs sorting"));
        assert!(xml.contains(r#"<varequal respident="response1">true_choice</varequal>"#));
    }

    #[test]
    /// A `false` answer marks the false choice as correct instead.
    fn false_answer_marks_false_choice_correct() {
        let xml = assessment_xml("a", &true_false_bank(false, "Q?")).expect("assessment renders");
        assert!(xml.contains(r#"<varequal respident="response1">false_choice</varequal>"#));
    }

    #[test]
    /// Angle brackets and ampersands from the prompt are XML-escaped, not raw.
    fn prompt_special_characters_are_escaped() {
        let xml = assessment_xml("a", &true_false_bank(true, "Is `a < b && c` valid?"))
            .expect("assessment renders");
        assert!(xml.contains("&lt;"));
        assert!(xml.contains("&amp;"));
        assert!(!xml.contains("a < b"));
    }

    #[test]
    /// The point value fills `points_possible`; whole numbers drop the decimal.
    fn points_are_formatted_into_metadata() {
        let xml = assessment_xml("a", &true_false_bank(true, "Q?")).expect("renders");
        assert!(xml.contains("<fieldentry>1</fieldentry>"));
        let mut fractional = true_false_bank(true, "Q?");
        if let Some(item) = fractional.items.first_mut() {
            item.points = 2.5;
        }
        let xml = assessment_xml("a", &fractional).expect("renders");
        assert!(xml.contains("<fieldentry>2.5</fieldentry>"));
    }

    #[test]
    /// A Markdown prompt is rendered to HTML and trimmed.
    fn prompt_html_renders_markdown() {
        assert_eq!(
            prompt_html("A **bold** word"),
            "<p>A <strong>bold</strong> word</p>"
        );
    }

    #[test]
    /// The manifest wires the resource ident and href into the resource nodes.
    fn manifest_references_assessment_resource() {
        let xml = manifest_xml("assessment_m", "assessment_m/assessment_m.xml");
        assert!(xml.contains(r#"identifier="assessment_m""#));
        assert!(xml.contains(r#"href="assessment_m/assessment_m.xml""#));
    }

    #[test]
    /// Feedback emits `<itemfeedback>` blocks and wires the display links.
    fn feedback_emits_itemfeedback_and_display_links() {
        let mut bank = true_false_bank(true, "Q?");
        if let Some(item) = bank.items.first_mut() {
            item.feedback = Feedback {
                general: Some("Recall the definition.".to_owned()),
                correct: Some("Correct!".to_owned()),
                incorrect: Some("Try again.".to_owned()),
            };
        }
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
        assert!(xml.contains(r#"<itemfeedback ident="correct_fb">"#));
        assert!(xml.contains(r#"<itemfeedback ident="incorrect_fb">"#));
        assert!(xml.contains(r#"linkrefid="correct_fb""#));
        assert!(xml.contains(r#"linkrefid="incorrect_fb""#));
        assert!(xml.contains("Correct!"));
        // The wrong choice (false, here) drives the incorrect-feedback arm.
        assert!(xml.contains(r#"<varequal respident="response1">false_choice</varequal>"#));
    }

    #[test]
    /// With no feedback, no feedback markup is emitted at all.
    fn absent_feedback_emits_no_markup() {
        let xml = assessment_xml("a", &true_false_bank(true, "Q?")).expect("renders");
        assert!(!xml.contains("<itemfeedback"));
        assert!(!xml.contains("displayfeedback"));
    }

    #[test]
    /// An explicit title is used for the item; absent falls back to the id.
    fn item_title_uses_title_then_falls_back_to_id() {
        let mut bank = true_false_bank(true, "Q?");
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"title="tf-1""#));
        if let Some(item) = bank.items.first_mut() {
            item.title = Some("Sorting basics".to_owned());
        }
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"title="Sorting basics""#));
    }

    #[test]
    /// With only correct feedback set, only its block and link are emitted.
    fn partial_feedback_emits_only_set_fields() {
        let mut bank = true_false_bank(true, "Q?");
        if let Some(item) = bank.items.first_mut() {
            item.feedback = Feedback {
                general: None,
                correct: Some("Correct!".to_owned()),
                incorrect: None,
            };
        }
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"<itemfeedback ident="correct_fb">"#));
        assert!(!xml.contains(r#"ident="general_fb""#));
        assert!(!xml.contains(r#"ident="incorrect_fb""#));
        assert!(xml.contains(r#"linkrefid="correct_fb""#));
        assert!(!xml.contains(r#"linkrefid="incorrect_fb""#));
    }

    #[test]
    /// General-only feedback emits the always-shown `continue="Yes"` arm alone.
    fn general_only_feedback_emits_general_arm() {
        let feedback = Feedback {
            general: Some("Recall the definition.".to_owned()),
            correct: None,
            incorrect: None,
        };
        let xml = resprocessing_xml(TRUE_CHOICE_IDENT, FALSE_CHOICE_IDENT, &feedback);
        assert!(xml.contains(r#"continue="Yes""#));
        assert!(xml.contains("<other/>"));
        assert!(xml.contains(r#"linkrefid="general_fb""#));
        assert!(!xml.contains(r#"linkrefid="correct_fb""#));
        assert!(!xml.contains(r#"linkrefid="incorrect_fb""#));
    }

    #[test]
    /// A false answer attaches correct feedback to the false choice, and the
    /// incorrect arm to the true choice.
    fn false_answer_swaps_feedback_choices() {
        let feedback = Feedback {
            general: None,
            correct: Some("Right.".to_owned()),
            incorrect: Some("Wrong.".to_owned()),
        };
        // answer == false => (correct, incorrect) = (false_choice, true_choice).
        let xml = resprocessing_xml(FALSE_CHOICE_IDENT, TRUE_CHOICE_IDENT, &feedback);
        assert!(xml.contains(r#"<varequal respident="response1">false_choice</varequal>"#));
        assert!(xml.contains(r#"<varequal respident="response1">true_choice</varequal>"#));
        assert!(xml.contains(r#"linkrefid="correct_fb""#));
        assert!(xml.contains(r#"linkrefid="incorrect_fb""#));
    }

    #[test]
    /// An unsupported question type reports a typed export error.
    fn unsupported_kind_errors() {
        let bank = ItemBank {
            name: "m".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "p".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::Ordering,
            }],
        };
        let err = to_qti(&bank).expect_err("ordering is unsupported");
        assert!(matches!(err, crate::Error::Export(_)));
    }

    #[test]
    /// All five predefined entities are escaped, with `&` handled first.
    fn escape_xml_encodes_all_specials() {
        assert_eq!(
            escape_xml("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&apos;&lt;/a&gt;"
        );
    }

    #[test]
    /// Non-identifier characters are folded to underscores; empty falls back.
    fn sanitize_ident_folds_and_defaults() {
        assert_eq!(sanitize_ident("tf.binary search"), "tf_binary_search");
        assert_eq!(sanitize_ident(""), "id");
    }
}
