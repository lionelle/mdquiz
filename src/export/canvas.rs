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

use std::fmt::Write as _;
use std::io::{Cursor, Write};

use pulldown_cmark::{Parser, html};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::Result;
use crate::model::{
    Blank, Choice, ChoiceSet, Feedback, FillInBlank, ItemBank, MatchMode, Question, QuestionKind,
    TrueFalse,
};

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

/// The scoring condition; `{{CONDITIONVAR}}` is the correct-answer match and
/// `{{CORRECT_FEEDBACK}}` is a display line or empty.
const SCORING_CONDITION: &str = r#"          <respcondition continue="No">
{{CONDITIONVAR}}            <setvar action="Set" varname="SCORE">100</setvar>
{{CORRECT_FEEDBACK}}          </respcondition>
"#;

/// A feedback-only condition; `{{CONDITIONVAR}}` is the match and `{{FEEDBACK}}`
/// a display-link line. Used for the incorrect-answer arm.
const FEEDBACK_CONDITION: &str = r#"          <respcondition continue="No">
{{CONDITIONVAR}}{{FEEDBACK}}          </respcondition>
"#;

/// A `<conditionvar>` matching a single response ident (`{{IDENT}}`).
const VAREQUAL_CONDITIONVAR: &str = r#"            <conditionvar>
              <varequal respident="response1">{{IDENT}}</varequal>
            </conditionvar>
"#;

/// A `<conditionvar>` matching anything *but* a single response ident.
const NOT_VAREQUAL_CONDITIONVAR: &str = r#"            <conditionvar>
              <not>
                <varequal respident="response1">{{IDENT}}</varequal>
              </not>
            </conditionvar>
"#;

/// A `<conditionvar>` matching the exact multi-answer set (`{{TERMS}}`).
const AND_CONDITIONVAR: &str = r"            <conditionvar>
              <and>
{{TERMS}}              </and>
            </conditionvar>
";

/// A `<conditionvar>` matching anything *but* the exact multi-answer set.
const NOT_AND_CONDITIONVAR: &str = r"            <conditionvar>
              <not>
                <and>
{{TERMS}}                </and>
              </not>
            </conditionvar>
";

/// The Canvas question type for a single-answer multiple-choice item.
const MC_QUESTION_TYPE: &str = "multiple_choice_question";
/// The Canvas question type for a multiple-answer item.
const MS_QUESTION_TYPE: &str = "multiple_answers_question";
/// The `rcardinality` for a single-answer response.
const CARDINALITY_SINGLE: &str = "Single";
/// The `rcardinality` for a multiple-answer response.
const CARDINALITY_MULTIPLE: &str = "Multiple";

/// A choice-based QTI item shared by multiple choice and multiple select;
/// `{{QUESTION_TYPE}}`, `{{RCARDINALITY}}`, and `{{CHOICES}}` are per-type.
const CHOICE_ITEM_TEMPLATE: &str = r#"      <item ident="{{ITEM_IDENT}}" title="{{ITEM_TITLE}}">
        <itemmetadata>
          <qtimetadata>
            <qtimetadatafield>
              <fieldlabel>question_type</fieldlabel>
              <fieldentry>{{QUESTION_TYPE}}</fieldentry>
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
          <response_lid ident="response1" rcardinality="{{RCARDINALITY}}">
            <render_choice>
{{CHOICES}}            </render_choice>
          </response_lid>
        </presentation>
{{RESPROCESSING}}{{ITEMFEEDBACK}}      </item>
"#;

/// One `<response_label>` option in a choice-based question.
const RESPONSE_LABEL_TEMPLATE: &str = r#"              <response_label ident="{{CHOICE_IDENT}}">
                <material><mattext texttype="text/html">{{CHOICE_TEXT}}</mattext></material>
              </response_label>
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

/// A fill-in-multiple-blanks QTI item; `{{RESPONSES}}` holds one `response_lid`
/// per blank and `{{CONDITIONS}}` the per-blank partial-credit scoring.
const FILL_IN_BLANK_ITEM_TEMPLATE: &str = r#"      <item ident="{{ITEM_IDENT}}" title="{{ITEM_TITLE}}">
        <itemmetadata>
          <qtimetadata>
            <qtimetadatafield>
              <fieldlabel>question_type</fieldlabel>
              <fieldentry>fill_in_multiple_blanks_question</fieldentry>
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
{{RESPONSES}}        </presentation>
        <resprocessing>
          <outcomes>
            <decvar maxvalue="100" minvalue="0" varname="SCORE" vartype="Decimal"/>
          </outcomes>
{{CONDITIONS}}        </resprocessing>
{{ITEMFEEDBACK}}      </item>
"#;

/// One blank's `<response_lid>`: its name plus its acceptable-answer labels.
const BLANK_RESPONSE_TEMPLATE: &str = r#"          <response_lid ident="{{RESPONSE_IDENT}}">
            <material>
              <mattext>{{BLANK_NAME}}</mattext>
            </material>
            <render_choice>
{{LABELS}}            </render_choice>
          </response_lid>
"#;

/// One acceptable answer for a blank, as a `<response_label>`.
const BLANK_ANSWER_LABEL_TEMPLATE: &str = r#"              <response_label ident="{{LABEL_IDENT}}">
                <material><mattext texttype="text/plain">{{ANSWER}}</mattext></material>
              </response_label>
"#;

/// One scoring condition: a matched blank answer adds its share of the score.
const BLANK_CONDITION_TEMPLATE: &str = r#"          <respcondition continue="Yes">
            <conditionvar>
              <varequal respident="{{RESPONSE_IDENT}}">{{LABEL_IDENT}}</varequal>
            </conditionvar>
            <setvar action="Add" varname="SCORE">{{PER_BLANK}}</setvar>
          </respcondition>
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

/// Reminders about content that can't fully round-trip through the Canvas
/// item-bank import and needs a manual fix afterward.
///
/// Currently this reports fill-in-the-blank blanks authored with `match: regex`:
/// they export as literal-text blanks (Canvas can't set the regex mode on
/// import), so the author must switch them to "Regular Expression Match" in the
/// New Quizzes editor. Empty when nothing needs attention.
#[must_use]
pub fn import_reminders(bank: &ItemBank) -> Vec<String> {
    let mut reminders = Vec::new();
    for question in &bank.items {
        let QuestionKind::FillInBlank(fitb) = &question.kind else {
            continue;
        };
        for blank in &fitb.blanks {
            if blank.match_mode == MatchMode::Regex {
                reminders.push(format!(
                    "question {:?}: set blank {:?} to \"Regular Expression Match\" in New Quizzes (it exported as literal text)",
                    question.id, blank.id
                ));
            }
        }
    }
    reminders
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
        QuestionKind::MultipleChoice(set) => Ok(multiple_choice_item_xml(question, set)),
        QuestionKind::MultipleSelect(set) => Ok(multiple_select_item_xml(question, set)),
        QuestionKind::FillInBlank(fitb) => Ok(fill_in_blank_item_xml(question, fitb)),
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
    let presentation = TRUE_FALSE_ITEM_TEMPLATE
        .replace("{{TRUE_IDENT}}", TRUE_CHOICE_IDENT)
        .replace("{{FALSE_IDENT}}", FALSE_CHOICE_IDENT);
    // The wrong answer is the single other choice.
    let correct_cv = varequal_conditionvar(correct);
    let incorrect_cv = varequal_conditionvar(incorrect);
    fill_item_shell(&presentation, question, &correct_cv, &incorrect_cv)
}

/// Render a single-answer multiple-choice item.
fn multiple_choice_item_xml(question: &Question, set: &ChoiceSet) -> String {
    let correct = correct_choice_ident(&set.choices);
    let presentation = choice_presentation(MC_QUESTION_TYPE, CARDINALITY_SINGLE, &set.choices);
    // Anything other than the correct choice is wrong.
    let correct_cv = varequal_conditionvar(&correct);
    let incorrect_cv = not_varequal_conditionvar(&correct);
    fill_item_shell(&presentation, question, &correct_cv, &incorrect_cv)
}

/// Render a multiple-answer item, scored all-or-nothing on the exact set.
fn multiple_select_item_xml(question: &Question, set: &ChoiceSet) -> String {
    let presentation = choice_presentation(MS_QUESTION_TYPE, CARDINALITY_MULTIPLE, &set.choices);
    // A correct answer selects every correct option and no incorrect one.
    let correct_cv = and_conditionvar(&set.choices);
    let incorrect_cv = not_and_conditionvar(&set.choices);
    fill_item_shell(&presentation, question, &correct_cv, &incorrect_cv)
}

/// Fill the shared choice-item template with its per-type presentation fields.
fn choice_presentation(question_type: &str, cardinality: &str, choices: &[Choice]) -> String {
    CHOICE_ITEM_TEMPLATE
        .replace("{{QUESTION_TYPE}}", question_type)
        .replace("{{RCARDINALITY}}", cardinality)
        // `{{CHOICES}}` is filled last so choice text cannot collide with a
        // placeholder above.
        .replace("{{CHOICES}}", &choices_xml(choices))
}

/// Render a fill-in-multiple-blanks item, scored per blank.
fn fill_in_blank_item_xml(question: &Question, fitb: &FillInBlank) -> String {
    let ordered = fitb.ordered(&question.prompt);
    fill_item_header(FILL_IN_BLANK_ITEM_TEMPLATE, question)
        .replace("{{RESPONSES}}", &blank_responses_xml(&ordered))
        .replace(
            "{{CONDITIONS}}",
            &blank_conditions_xml(&ordered, &question.feedback),
        )
        .replace(
            "{{ITEMFEEDBACK}}",
            &general_itemfeedback_xml(&question.feedback),
        )
        // `{{PROMPT}}` is filled last so authored text is never re-scanned.
        .replace("{{PROMPT}}", &blank_prompt_html(&question.prompt, &ordered))
}

/// The QTI response identifier for a blank named `id`.
fn response_ident(id: &str) -> String {
    sanitize_ident(&format!("response_{id}"))
}

/// The QTI label identifier for the `index`th answer of blank `id`.
fn answer_ident(id: &str, index: usize) -> String {
    sanitize_ident(&format!("{id}_answer_{index}"))
}

/// Render the prompt HTML, rewriting each `{{name}}` marker to Canvas's
/// `[name]` blank reference.
fn blank_prompt_html(prompt: &str, blanks: &[&Blank]) -> String {
    let mut html = escaped_html(prompt);
    for blank in blanks {
        let reference = ["[", &blank.id, "]"].concat();
        html = html.replace(&crate::model::blank_marker(&blank.id), &reference);
    }
    html
}

/// Render one `<response_lid>` per blank, each listing its answer labels.
fn blank_responses_xml(blanks: &[&Blank]) -> String {
    let mut out = String::new();
    for blank in blanks {
        out.push_str(
            &BLANK_RESPONSE_TEMPLATE
                .replace("{{RESPONSE_IDENT}}", &response_ident(&blank.id))
                .replace("{{BLANK_NAME}}", &escape_xml(&blank.id))
                .replace("{{LABELS}}", &blank_labels_xml(blank)),
        );
    }
    out
}

/// Render one `<response_label>` per acceptable answer of `blank`.
fn blank_labels_xml(blank: &Blank) -> String {
    let mut out = String::new();
    for (index, answer) in blank.answers.iter().enumerate() {
        out.push_str(
            &BLANK_ANSWER_LABEL_TEMPLATE
                .replace("{{LABEL_IDENT}}", &answer_ident(&blank.id, index))
                .replace("{{ANSWER}}", &escape_xml(answer)),
        );
    }
    out
}

/// Render the scoring conditions: general feedback plus per-blank partial credit.
fn blank_conditions_xml(blanks: &[&Blank], feedback: &Feedback) -> String {
    let mut out = general_condition_xml(feedback);
    let per_blank = per_blank_score(blanks.len());
    for blank in blanks {
        for index in 0..blank.answers.len() {
            out.push_str(
                &BLANK_CONDITION_TEMPLATE
                    .replace("{{RESPONSE_IDENT}}", &response_ident(&blank.id))
                    .replace("{{LABEL_IDENT}}", &answer_ident(&blank.id, index))
                    .replace("{{PER_BLANK}}", &per_blank),
            );
        }
    }
    out
}

/// The percentage each blank contributes, split evenly across the blanks.
///
/// Clamped to at least one blank so the divisor is never zero (an empty or
/// absurdly large blank set falls back to full credit).
fn per_blank_score(count: usize) -> String {
    let count = u16::try_from(count).map_or(1.0, f64::from).max(1.0);
    format!("{:.2}", 100.0 / count)
}

/// Render the general-feedback `<itemfeedback>` block, if any.
///
/// Fill-in-the-blank only wires general feedback: Canvas keeps answer-level
/// (correct/incorrect) feedback for multiple choice only.
fn general_itemfeedback_xml(feedback: &Feedback) -> String {
    feedback
        .general
        .as_deref()
        .map_or_else(String::new, |text| {
            itemfeedback_block(GENERAL_FB_IDENT, text)
        })
}

/// Fill the shared item shell (metadata, scoring, feedback, prompt) around a
/// per-type presentation.
///
/// `correct_cv`/`incorrect_cv` are the `<conditionvar>` blocks for a right and
/// wrong answer. The presentation-specific placeholders are already resolved in
/// `template`; `{{PROMPT}}` is substituted last so authored text is not
/// re-scanned.
fn fill_item_shell(
    template: &str,
    question: &Question,
    correct_cv: &str,
    incorrect_cv: &str,
) -> String {
    let feedback = &question.feedback;
    fill_item_header(template, question)
        .replace(
            "{{RESPROCESSING}}",
            &resprocessing_xml(correct_cv, incorrect_cv, feedback),
        )
        .replace("{{ITEMFEEDBACK}}", &itemfeedback_xml(feedback))
        .replace("{{PROMPT}}", &escaped_html(&question.prompt))
}

/// Substitute the item-header fields (ident, title, points) shared by every
/// item template. Callers fill in the remaining per-type placeholders.
fn fill_item_header(template: &str, question: &Question) -> String {
    let title = question.title.as_deref().unwrap_or(&question.id);
    template
        .replace("{{ITEM_IDENT}}", &sanitize_ident(&question.id))
        .replace("{{ITEM_TITLE}}", &escape_xml(title))
        .replace("{{POINTS}}", &question.points.to_string())
}

/// The generated `response_label` ident for the choice at `index`.
fn choice_ident(index: usize) -> String {
    format!("choice_{index}")
}

/// The ident of the first correct choice, for scoring (falls back to `choice_0`).
fn correct_choice_ident(choices: &[Choice]) -> String {
    let index = choices
        .iter()
        .position(|choice| choice.correct)
        .unwrap_or(0);
    choice_ident(index)
}

/// Render the `<response_label>` list for a choice-based question.
fn choices_xml(choices: &[Choice]) -> String {
    let mut out = String::new();
    for (index, choice) in choices.iter().enumerate() {
        out.push_str(
            &RESPONSE_LABEL_TEMPLATE
                .replace("{{CHOICE_IDENT}}", &choice_ident(index))
                .replace("{{CHOICE_TEXT}}", &escaped_html(&choice.text)),
        );
    }
    out
}

/// A `<conditionvar>` matching a single response ident.
fn varequal_conditionvar(ident: &str) -> String {
    VAREQUAL_CONDITIONVAR.replace("{{IDENT}}", ident)
}

/// A `<conditionvar>` matching anything but a single response ident.
fn not_varequal_conditionvar(ident: &str) -> String {
    NOT_VAREQUAL_CONDITIONVAR.replace("{{IDENT}}", ident)
}

/// A `<conditionvar>` matching the exact set of correct/incorrect choices.
fn and_conditionvar(choices: &[Choice]) -> String {
    AND_CONDITIONVAR.replace("{{TERMS}}", &correctness_terms(choices))
}

/// A `<conditionvar>` matching anything but the exact correct/incorrect set.
fn not_and_conditionvar(choices: &[Choice]) -> String {
    NOT_AND_CONDITIONVAR.replace("{{TERMS}}", &correctness_terms(choices))
}

/// One `<varequal>`/`<not>` term per choice: selected iff correct.
fn correctness_terms(choices: &[Choice]) -> String {
    let mut out = String::new();
    for (index, choice) in choices.iter().enumerate() {
        let ident = choice_ident(index);
        // Writing to a `String` is infallible, so the result is discarded.
        let _ = if choice.correct {
            writeln!(
                out,
                "                <varequal respident=\"response1\">{ident}</varequal>"
            )
        } else {
            writeln!(
                out,
                "                <not><varequal respident=\"response1\">{ident}</varequal></not>"
            )
        };
    }
    out
}

/// Build the resprocessing block: scoring plus any feedback display arms.
///
/// `correct_cv`/`incorrect_cv` are the `<conditionvar>` blocks for a right and
/// wrong answer; the incorrect arm is emitted only when incorrect feedback is
/// present.
fn resprocessing_xml(correct_cv: &str, incorrect_cv: &str, feedback: &Feedback) -> String {
    let mut conditions = general_condition_xml(feedback);
    let correct_feedback = if feedback.correct.is_some() {
        display_feedback(CORRECT_FB_IDENT)
    } else {
        String::new()
    };
    conditions.push_str(
        &SCORING_CONDITION
            .replace("{{CONDITIONVAR}}", correct_cv)
            .replace("{{CORRECT_FEEDBACK}}", &correct_feedback),
    );
    if feedback.incorrect.is_some() {
        conditions.push_str(
            &FEEDBACK_CONDITION
                .replace("{{CONDITIONVAR}}", incorrect_cv)
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
            out.push_str(&itemfeedback_block(ident, text));
        }
    }
    out
}

/// One `<itemfeedback>` block holding `text` (rendered from Markdown) under
/// `ident`.
fn itemfeedback_block(ident: &str, text: &str) -> String {
    ITEM_FEEDBACK_TEMPLATE
        .replace("{{IDENT}}", ident)
        .replace("{{HTML}}", &escaped_html(text))
}

/// The always-shown condition that displays general feedback, or empty when
/// there is none.
fn general_condition_xml(feedback: &Feedback) -> String {
    if feedback.general.is_some() {
        GENERAL_CONDITION.replace("{{FEEDBACK}}", &display_feedback(GENERAL_FB_IDENT))
    } else {
        String::new()
    }
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

    /// Build a one-question multiple-choice bank; `correct` marks which of the
    /// three options is right.
    fn multiple_choice_bank(correct: usize, feedback: Feedback) -> ItemBank {
        let choices = (0..3usize)
            .map(|index| Choice {
                text: format!("Option {index}"),
                correct: index == correct,
            })
            .collect();
        ItemBank {
            name: "Module 1".to_owned(),
            items: vec![Question {
                id: "mc-1".to_owned(),
                title: None,
                prompt: "Pick one.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback,
                kind: QuestionKind::MultipleChoice(ChoiceSet { choices }),
            }],
        }
    }

    #[test]
    /// A multiple-choice item renders single-select choices and scores the mark.
    fn multiple_choice_renders_and_scores_marked_choice() {
        let xml = assessment_xml("a", &multiple_choice_bank(2, Feedback::default()))
            .expect("assessment renders");
        assert!(xml.contains("multiple_choice_question"));
        assert!(xml.contains(r#"rcardinality="Single""#));
        assert!(xml.contains(r#"<response_label ident="choice_0">"#));
        assert!(xml.contains(r#"<response_label ident="choice_2">"#));
        assert!(xml.contains("Option 1"));
        // The correct choice (index 2) is the 100-point response.
        assert!(xml.contains(r#"<varequal respident="response1">choice_2</varequal>"#));
        // No incorrect feedback here, so no `<not>` arm is emitted.
        assert!(!xml.contains("<not>"));
    }

    #[test]
    /// A multiple-select item uses Multiple cardinality and all-or-nothing
    /// scoring: select every correct option and no incorrect one.
    fn multiple_select_uses_multiple_cardinality_and_and_scoring() {
        let choice = |text: &str, correct: bool| Choice {
            text: text.to_owned(),
            correct,
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "ms".to_owned(),
                title: None,
                prompt: "Select all.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::MultipleSelect(ChoiceSet {
                    choices: vec![choice("A", true), choice("B", false), choice("C", true)],
                }),
            }],
        };
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("multiple_answers_question"));
        assert!(xml.contains(r#"rcardinality="Multiple""#));
        assert!(xml.contains("<and>"));
        assert!(xml.contains(r#"<varequal respident="response1">choice_0</varequal>"#));
        assert!(xml.contains(r#"<varequal respident="response1">choice_2</varequal>"#));
        assert!(xml.contains(r#"<not><varequal respident="response1">choice_1</varequal></not>"#));
    }

    /// Build a multiple-select bank from `(text, correct)` pairs and feedback.
    fn multiple_select_bank(choices: &[(&str, bool)], feedback: Feedback) -> ItemBank {
        let choices = choices
            .iter()
            .map(|(text, correct)| Choice {
                text: (*text).to_owned(),
                correct: *correct,
            })
            .collect();
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "ms".to_owned(),
                title: None,
                prompt: "Select all.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback,
                kind: QuestionKind::MultipleSelect(ChoiceSet { choices }),
            }],
        }
    }

    #[test]
    /// Multiple-select incorrect feedback wraps the exact set in `<not><and>`.
    fn multiple_select_incorrect_feedback_uses_not_and_condition() {
        let feedback = Feedback {
            general: Some("Recall which need ordering.".to_owned()),
            correct: Some("Yes.".to_owned()),
            incorrect: Some("No.".to_owned()),
        };
        let bank = multiple_select_bank(&[("A", true), ("B", false), ("C", true)], feedback);
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("<not>"));
        assert!(xml.contains("<and>"));
        assert!(xml.contains(r#"linkrefid="incorrect_fb""#));
        assert!(xml.contains(r#"<itemfeedback ident="correct_fb">"#));
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
    }

    #[test]
    /// An all-correct multiple-select emits only `<varequal>` terms, no `<not>`.
    fn multiple_select_all_correct_has_no_not_terms() {
        let bank = multiple_select_bank(&[("A", true), ("B", true)], Feedback::default());
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"<varequal respident="response1">choice_0</varequal>"#));
        assert!(xml.contains(r#"<varequal respident="response1">choice_1</varequal>"#));
        assert!(!xml.contains("<not>"));
    }

    /// Build a two-blank fill-in-the-blank bank with the given feedback.
    fn fill_in_blank_bank(feedback: Feedback) -> ItemBank {
        let mk_blank = |id: &str, answers: &[&str]| Blank {
            id: id.to_owned(),
            answers: answers.iter().map(|answer| (*answer).to_owned()).collect(),
            match_mode: MatchMode::CaseInsensitive,
        };
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "fitb".to_owned(),
                title: None,
                prompt: "HTTP {{method}} returns {{code}}.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback,
                kind: QuestionKind::FillInBlank(FillInBlank {
                    blanks: vec![
                        mk_blank("method", &["GET"]),
                        mk_blank("code", &["404", "Not Found"]),
                    ],
                }),
            }],
        }
    }

    #[test]
    /// Fill-in-the-blank renders one response per blank, inline `[name]`
    /// markers, and per-blank partial-credit scoring.
    fn fill_in_blank_renders_blanks_and_scoring() {
        let xml = assessment_xml("a", &fill_in_blank_bank(Feedback::default())).expect("renders");
        assert!(xml.contains("fill_in_multiple_blanks_question"));
        // Markers rewritten from {{name}} to Canvas's [name].
        assert!(xml.contains("[method]"));
        assert!(xml.contains("[code]"));
        assert!(!xml.contains("{{method}}"));
        assert!(xml.contains(r#"<response_lid ident="response_method">"#));
        assert!(xml.contains(r#"<response_lid ident="response_code">"#));
        assert!(xml.contains("Not Found"));
        // Two blanks -> 50.00 each, added on a matching answer.
        assert!(xml.contains(r#"<setvar action="Add" varname="SCORE">50.00</setvar>"#));
        assert!(xml.contains(r#"<varequal respident="response_code">code_answer_1</varequal>"#));
    }

    #[test]
    /// Blanks are emitted in prompt order, not the model's storage order.
    fn fill_in_blank_orders_responses_by_prompt() {
        let mk_blank = |id: &str| Blank {
            id: id.to_owned(),
            answers: vec!["x".to_owned()],
            match_mode: MatchMode::CaseInsensitive,
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "fitb".to_owned(),
                title: None,
                prompt: "First {{alpha}} then {{beta}}.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                // Stored in the reverse of prompt order.
                kind: QuestionKind::FillInBlank(FillInBlank {
                    blanks: vec![mk_blank("beta"), mk_blank("alpha")],
                }),
            }],
        };
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.find("response_alpha") < xml.find("response_beta"));
    }

    #[test]
    /// Per-blank credit splits 100% evenly across the blanks.
    fn per_blank_score_splits_evenly() {
        assert_eq!(per_blank_score(1), "100.00");
        assert_eq!(per_blank_score(2), "50.00");
        assert_eq!(per_blank_score(3), "33.33");
    }

    #[test]
    /// XML-special characters in a blank answer are escaped, not emitted raw.
    fn fill_in_blank_escapes_answer_text() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "fitb".to_owned(),
                title: None,
                prompt: "Value is {{v}}.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::FillInBlank(FillInBlank {
                    blanks: vec![Blank {
                        id: "v".to_owned(),
                        answers: vec!["a < b & c".to_owned()],
                        match_mode: MatchMode::CaseInsensitive,
                    }],
                }),
            }],
        };
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("&lt;"));
        assert!(xml.contains("&amp;"));
        assert!(!xml.contains("a < b"));
    }

    #[test]
    /// A regex blank exports as literal text and yields a manual-fix reminder.
    fn regex_blank_exports_literal_and_warns() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "fitb".to_owned(),
                title: None,
                prompt: "A 3-digit code: {{n}}.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::FillInBlank(FillInBlank {
                    blanks: vec![Blank {
                        id: "n".to_owned(),
                        answers: vec![r"\d{3}".to_owned()],
                        match_mode: MatchMode::Regex,
                    }],
                }),
            }],
        };
        // Exports as a standard fill-in-blank carrying the pattern as literal text.
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("fill_in_multiple_blanks_question"));
        assert!(xml.contains(r"\d{3}"));
        // And it surfaces a manual-fix reminder naming the question and blank.
        let reminders = import_reminders(&bank);
        assert_eq!(reminders.len(), 1);
        let note = reminders.first().expect("one reminder");
        assert!(note.contains("fitb") && note.contains("\"n\""));
        // A non-regex fill-in-blank bank produces no reminders.
        assert!(import_reminders(&fill_in_blank_bank(Feedback::default())).is_empty());
    }

    #[test]
    /// A bank with no fill-in-the-blank questions yields no import reminders.
    fn non_fitb_bank_has_no_reminders() {
        assert!(import_reminders(&true_false_bank(true, "Q?")).is_empty());
    }

    #[test]
    /// Fill-in-the-blank carries general feedback (answer-level is MC-only).
    fn fill_in_blank_general_feedback_is_emitted() {
        let feedback = Feedback {
            general: Some("Recall HTTP semantics.".to_owned()),
            correct: None,
            incorrect: None,
        };
        let xml = assessment_xml("a", &fill_in_blank_bank(feedback)).expect("renders");
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
        assert!(xml.contains(r#"linkrefid="general_fb""#));
    }

    #[test]
    /// XML-special characters in choice text are escaped, not emitted raw.
    fn multiple_choice_escapes_choice_text() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "mc".to_owned(),
                title: None,
                prompt: "Pick.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::MultipleChoice(ChoiceSet {
                    choices: vec![
                        Choice {
                            text: "a < b & c".to_owned(),
                            correct: true,
                        },
                        Choice {
                            text: "plain".to_owned(),
                            correct: false,
                        },
                    ],
                }),
            }],
        };
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("&lt;"));
        assert!(xml.contains("&amp;"));
        assert!(!xml.contains("a < b"));
    }

    #[test]
    /// Incorrect feedback on a multiple-choice item uses a `<not>` condition.
    fn multiple_choice_incorrect_feedback_uses_not_condition() {
        let feedback = Feedback {
            general: None,
            correct: Some("Yes.".to_owned()),
            incorrect: Some("No.".to_owned()),
        };
        let xml = assessment_xml("a", &multiple_choice_bank(1, feedback)).expect("renders");
        assert!(xml.contains("<not>"));
        assert!(xml.contains(r#"linkrefid="incorrect_fb""#));
        assert!(xml.contains(r#"<itemfeedback ident="correct_fb">"#));
    }

    #[test]
    /// A multiple-choice bank exports as a valid two-file zip package.
    fn multiple_choice_exports_valid_zip() {
        let bytes = to_qti(&multiple_choice_bank(0, Feedback::default())).expect("export");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        assert_eq!(archive.len(), 2);
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
        // No incorrect feedback, so the incorrect conditionvar is unused (empty).
        let xml = resprocessing_xml(&varequal_conditionvar(TRUE_CHOICE_IDENT), "", &feedback);
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
        let mut bank = true_false_bank(false, "Q?");
        if let Some(item) = bank.items.first_mut() {
            item.feedback = Feedback {
                general: None,
                correct: Some("Right.".to_owned()),
                incorrect: Some("Wrong.".to_owned()),
            };
        }
        let xml = assessment_xml("a", &bank).expect("renders");
        // answer == false => correct scores false_choice, incorrect arm hits true.
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
