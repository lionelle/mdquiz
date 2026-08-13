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

use pulldown_cmark::{Event, Options, Parser, Tag, html};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::Result;
use crate::model::{
    Blank, Choice, ChoiceSet, Feedback, FillInBlank, ItemBank, MatchMode, MatchPair, Matching,
    MultipleSelect, Ordering, Question, QuestionKind, ScoringMode, TrueFalse, is_local_image,
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
{{IMAGE_RESOURCES}}  </resources>
</manifest>
"#;

/// The package directory bundled local images live in; the manifest href and
/// the zip entry path must both use it.
const WEB_RESOURCES_DIR: &str = "web_resources";

/// One bundled-image web-content resource in the manifest.
const IMAGE_RESOURCE_TEMPLATE: &str = r#"    <resource identifier="{{IDENT}}" type="webcontent" href="{{DIR}}/{{PATH}}">
      <file href="{{DIR}}/{{PATH}}"/>
    </resource>
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

/// One partial-credit arm: selecting choice `{{IDENT}}` adjusts the score by
/// `{{VALUE}}` via `{{ACTION}}` (`Add` for correct, `Subtract` for incorrect).
const PARTIAL_CONDITION: &str = r#"          <respcondition continue="Yes">
            <conditionvar>
              <varequal respident="response1">{{IDENT}}</varequal>
            </conditionvar>
            <setvar action="{{ACTION}}" varname="SCORE">{{VALUE}}</setvar>
          </respcondition>
"#;

/// The Canvas question type for a single-answer multiple-choice item.
const MC_QUESTION_TYPE: &str = "multiple_choice_question";
/// The Canvas question type for a multiple-answer item.
const MS_QUESTION_TYPE: &str = "multiple_answers_question";
/// The Canvas question type for a fill-in-multiple-blanks item.
const FITB_QUESTION_TYPE: &str = "fill_in_multiple_blanks_question";
/// The Canvas question type for a matching item.
const MATCHING_QUESTION_TYPE: &str = "matching_question";
/// The `rcardinality` for a single-answer response.
const CARDINALITY_SINGLE: &str = "Single";
/// The `rcardinality` for a multiple-answer response.
const CARDINALITY_MULTIPLE: &str = "Multiple";
/// The `<setvar>` action that adds to the score (correct partial-credit choice).
const SETVAR_ADD: &str = "Add";
/// The `<setvar>` action that subtracts (incorrect partial-credit choice).
const SETVAR_SUBTRACT: &str = "Subtract";

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

/// A `response_lid`-based QTI item (fill-in-the-blank or matching);
/// `{{QUESTION_TYPE}}` selects the kind, `{{RESPONSES}}` holds one `response_lid`
/// per blank or left prompt, and `{{CONDITIONS}}` the per-item partial scoring.
const RESPONSE_ITEM_TEMPLATE: &str = r#"      <item ident="{{ITEM_IDENT}}" title="{{ITEM_TITLE}}">
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

/// One scoring condition: selecting `{{IDENT}}` for `{{RESPIDENT}}` adds its
/// `{{VALUE}}` share of the score. Shared by fill-in-the-blank and matching.
const ADD_CONDITION_TEMPLATE: &str = r#"          <respcondition continue="Yes">
            <conditionvar>
              <varequal respident="{{RESPIDENT}}">{{IDENT}}</varequal>
            </conditionvar>
            <setvar action="Add" varname="SCORE">{{VALUE}}</setvar>
          </respcondition>
"#;

/// One left prompt's `<response_lid>`: its text plus the shared right options.
const MATCH_RESPONSE_TEMPLATE: &str = r#"          <response_lid ident="{{RESPONSE_IDENT}}">
            <material>
              <mattext texttype="text/plain">{{LEFT}}</mattext>
            </material>
            <render_choice>
{{OPTIONS}}            </render_choice>
          </response_lid>
"#;

/// One right-hand option, shared across every left prompt's `render_choice`.
const MATCH_OPTION_TEMPLATE: &str = r#"              <response_label ident="{{OPTION_IDENT}}">
                <material><mattext>{{RIGHT}}</mattext></material>
              </response_label>
"#;

/// An ordering QTI item. `{{ANSWER_IDS}}` lists every option ident in display
/// order, `{{OPTIONS}}` the shuffled draggable labels, and `{{CONDITIONS}}` the
/// single all-or-nothing correct-order condition.
const ORDERING_ITEM_TEMPLATE: &str = r#"      <item ident="{{ITEM_IDENT}}" title="{{ITEM_TITLE}}">
        <itemmetadata>
          <qtimetadata>
            <qtimetadatafield>
              <fieldlabel>question_type</fieldlabel>
              <fieldentry>ordering_question</fieldentry>
            </qtimetadatafield>
            <qtimetadatafield>
              <fieldlabel>points_possible</fieldlabel>
              <fieldentry>{{POINTS}}</fieldentry>
            </qtimetadatafield>
            <qtimetadatafield>
              <fieldlabel>original_answer_ids</fieldlabel>
              <fieldentry>{{ANSWER_IDS}}</fieldentry>
            </qtimetadatafield>
          </qtimetadata>
        </itemmetadata>
        <presentation>
          <material>
            <mattext texttype="text/html">{{PROMPT}}</mattext>
          </material>
          <response_lid ident="response1" rcardinality="Ordered">
            <render_extension>
              <material position="top">
                <mattext/>
              </material>
              <ims_render_object shuffle="No">
                <flow_label>
{{OPTIONS}}                </flow_label>
              </ims_render_object>
              <material position="bottom">
                <mattext/>
              </material>
            </render_extension>
          </response_lid>
        </presentation>
        <resprocessing>
          <outcomes>
            <decvar defaultval="1" varname="ORDERSCORE" vartype="Integer"/>
          </outcomes>
{{CONDITIONS}}        </resprocessing>
{{ITEMFEEDBACK}}      </item>
"#;

/// One draggable ordering option, rendered as HTML like a choice label.
const ORDER_OPTION_TEMPLATE: &str = r#"                <response_label ident="{{OPTION_IDENT}}">
                  <material>
                    <mattext texttype="text/html">{{TEXT}}</mattext>
                  </material>
                </response_label>
"#;

/// One position in the correct-order condition: option `{{OPTION_IDENT}}`.
const ORDER_VAREQUAL_TEMPLATE: &str =
    "              <varequal respident=\"response1\">{{OPTION_IDENT}}</varequal>\n";

/// The all-or-nothing correct-order condition: every option in authored order
/// scores full marks. `{{VAREQUALS}}` is the ordered `varequal` list.
const ORDER_CONDITION_TEMPLATE: &str = r#"          <respcondition continue="No">
            <conditionvar>
{{VAREQUALS}}            </conditionvar>
            <setvar action="Set" varname="SCORE">100</setvar>
          </respcondition>
"#;

/// Render `bank` as the bytes of a Canvas New Quizzes QTI package.
///
/// `images` supplies the bytes for local images referenced by the questions
/// (see [`local_image_paths`]); each is bundled under `web_resources/` and
/// referenced via `$IMS-CC-FILEBASE$`. Pass an empty slice for none.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] if the zip package cannot be built.
pub fn to_qti(bank: &ItemBank, images: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let assessment_ident = format!("assessment_{}", sanitize_ident(&bank.name));
    let href = format!("{assessment_ident}/{assessment_ident}.xml");
    let assessment = assessment_xml(&assessment_ident, bank)?;
    let manifest = manifest_xml(&assessment_ident, &href, images);
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("imsmanifest.xml".to_owned(), manifest.into_bytes()),
        (href, assessment.into_bytes()),
    ];
    for (path, bytes) in images {
        files.push((format!("{WEB_RESOURCES_DIR}/{path}"), bytes.clone()));
    }
    let entries: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    zip_package(&entries)
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

/// Fill the manifest with the assessment resource and any bundled-image
/// resources.
fn manifest_xml(resource_ident: &str, href: &str, images: &[(String, Vec<u8>)]) -> String {
    let mut image_resources = String::new();
    for (path, _) in images {
        image_resources.push_str(
            &IMAGE_RESOURCE_TEMPLATE
                .replace("{{IDENT}}", &sanitize_ident(&format!("res_{path}")))
                .replace("{{DIR}}", WEB_RESOURCES_DIR)
                .replace("{{PATH}}", &escape_xml(path)),
        );
    }
    MANIFEST_TEMPLATE
        .replace("{{RESOURCE_IDENT}}", &escape_xml(resource_ident))
        .replace("{{IMAGE_RESOURCES}}", &image_resources)
        .replace("{{HREF}}", &escape_xml(href))
}

/// Render the assessment document, one item per question.
///
/// # Errors
///
/// Currently infallible; the `Result` is retained because [`item_xml`] keeps a
/// fallible signature for future `#[non_exhaustive]` [`QuestionKind`] variants.
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
/// Currently infallible; the `Result` is retained because [`QuestionKind`] is
/// `#[non_exhaustive]`, so a future kind this exporter cannot yet represent can
/// be rejected as a new arm without a signature change rippling to every caller.
#[expect(
    clippy::unnecessary_wraps,
    reason = "fallible signature reserved for future non_exhaustive QuestionKind variants"
)]
fn item_xml(question: &Question) -> Result<String> {
    match &question.kind {
        QuestionKind::TrueFalse(answer) => Ok(true_false_item_xml(question, answer)),
        QuestionKind::MultipleChoice(set) => Ok(multiple_choice_item_xml(question, set)),
        QuestionKind::MultipleSelect(set) => Ok(multiple_select_item_xml(question, set)),
        QuestionKind::FillInBlank(fitb) => Ok(fill_in_blank_item_xml(question, fitb)),
        QuestionKind::Matching(matching) => Ok(matching_item_xml(question, matching)),
        QuestionKind::Ordering(ordering) => Ok(ordering_item_xml(question, ordering)),
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

/// Render a multiple-answer item, scored per its [`ScoringMode`].
fn multiple_select_item_xml(question: &Question, select: &MultipleSelect) -> String {
    let presentation = choice_presentation(MS_QUESTION_TYPE, CARDINALITY_MULTIPLE, &select.choices);
    match select.scoring {
        ScoringMode::AllOrNothing => {
            // Full marks only for the exact set: every correct, no incorrect.
            let correct_cv = and_conditionvar(&select.choices);
            let incorrect_cv = not_and_conditionvar(&select.choices);
            fill_item_shell(&presentation, question, &correct_cv, &incorrect_cv)
        }
        ScoringMode::Partial => fill_choice_item(
            &presentation,
            question,
            &partial_resprocessing_xml(&select.choices, &question.feedback),
            &general_itemfeedback_xml(&question.feedback),
        ),
    }
}

/// Build partial-credit resprocessing: each correct choice adds an even share,
/// each incorrect choice subtracts one; the outcome is clamped to zero.
fn partial_resprocessing_xml(choices: &[Choice], feedback: &Feedback) -> String {
    let correct = choices.iter().filter(|choice| choice.correct).count();
    let add = even_share_score(correct);
    let subtract = even_share_score(choices.len() - correct);
    let mut conditions = general_condition_xml(feedback);
    for (index, choice) in choices.iter().enumerate() {
        let (action, value) = if choice.correct {
            (SETVAR_ADD, &add)
        } else {
            (SETVAR_SUBTRACT, &subtract)
        };
        conditions.push_str(
            &PARTIAL_CONDITION
                .replace("{{IDENT}}", &choice_ident(index))
                .replace("{{ACTION}}", action)
                .replace("{{VALUE}}", value),
        );
    }
    RESPROCESSING_TEMPLATE.replace("{{CONDITIONS}}", &conditions)
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

/// Fill the shared `response_lid` item body (metadata, scoring, feedback, and
/// prompt) around a per-type set of responses and scoring conditions.
///
/// `prompt_html` is passed pre-rendered and substituted last so authored text
/// is never re-scanned for placeholders.
fn fill_response_item(
    question_type: &str,
    question: &Question,
    responses: &str,
    conditions: &str,
    prompt_html: &str,
) -> String {
    fill_item_header(RESPONSE_ITEM_TEMPLATE, question)
        .replace("{{QUESTION_TYPE}}", question_type)
        .replace("{{RESPONSES}}", responses)
        .replace("{{CONDITIONS}}", conditions)
        .replace(
            "{{ITEMFEEDBACK}}",
            &general_itemfeedback_xml(&question.feedback),
        )
        .replace("{{PROMPT}}", prompt_html)
}

/// Render a fill-in-multiple-blanks item, scored per blank.
fn fill_in_blank_item_xml(question: &Question, fitb: &FillInBlank) -> String {
    let ordered = fitb.ordered(&question.prompt);
    fill_response_item(
        FITB_QUESTION_TYPE,
        question,
        &blank_responses_xml(&ordered),
        &blank_conditions_xml(&ordered, &question.feedback),
        &blank_prompt_html(&question.prompt, &ordered),
    )
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
    let per_blank = even_share_score(blanks.len());
    for blank in blanks {
        for index in 0..blank.answers.len() {
            out.push_str(
                &ADD_CONDITION_TEMPLATE
                    .replace("{{RESPIDENT}}", &response_ident(&blank.id))
                    .replace("{{IDENT}}", &answer_ident(&blank.id, index))
                    .replace("{{VALUE}}", &per_blank),
            );
        }
    }
    out
}

/// An even `1/count` share of 100%, as a two-decimal string.
///
/// Used for per-blank fill-in-the-blank credit, per-choice partial credit, and
/// per-pair matching credit. Clamped to at least one so the divisor is never
/// zero (an empty or absurdly large `count` falls back to full credit).
fn even_share_score(count: usize) -> String {
    let count = u16::try_from(count).map_or(1.0, f64::from).max(1.0);
    format!("{:.2}", 100.0 / count)
}

/// Render the general-feedback `<itemfeedback>` block, if any.
///
/// The kinds that wire general feedback only — fill-in-the-blank, matching,
/// ordering, and partial-scored multiple-select — use this; Canvas keeps
/// answer-level (correct/incorrect) feedback for multiple choice only.
fn general_itemfeedback_xml(feedback: &Feedback) -> String {
    feedback
        .general
        .as_deref()
        .map_or_else(String::new, |text| {
            itemfeedback_block(GENERAL_FB_IDENT, text)
        })
}

/// Render a matching item: one response per left prompt, scored per pair.
fn matching_item_xml(question: &Question, matching: &Matching) -> String {
    let options = matching.options();
    let options_xml = match_options_xml(&options);
    fill_response_item(
        MATCHING_QUESTION_TYPE,
        question,
        &match_responses_xml(&matching.pairs, &options_xml),
        &match_conditions_xml(matching, &options, &question.feedback),
        &escaped_html(&question.prompt),
    )
}

/// The response identifier for the left prompt at `index`.
fn match_response_ident(index: usize) -> String {
    format!("response_{index}")
}

/// The option identifier for the right-hand option at `index`.
fn match_option_ident(index: usize) -> String {
    format!("answer_{index}")
}

/// Render the shared right-hand options (used in every left's `render_choice`).
fn match_options_xml(options: &[&str]) -> String {
    let mut out = String::new();
    for (index, right) in options.iter().enumerate() {
        out.push_str(
            &MATCH_OPTION_TEMPLATE
                .replace("{{OPTION_IDENT}}", &match_option_ident(index))
                .replace("{{RIGHT}}", &escape_xml(right)),
        );
    }
    out
}

/// Render one `<response_lid>` per left prompt, each offering all options.
fn match_responses_xml(pairs: &[MatchPair], options_xml: &str) -> String {
    let mut out = String::new();
    for (index, pair) in pairs.iter().enumerate() {
        out.push_str(
            &MATCH_RESPONSE_TEMPLATE
                .replace("{{RESPONSE_IDENT}}", &match_response_ident(index))
                .replace("{{LEFT}}", &escape_xml(&pair.left))
                .replace("{{OPTIONS}}", options_xml),
        );
    }
    out
}

/// Render the scoring conditions: general feedback plus per-pair partial credit.
fn match_conditions_xml(matching: &Matching, options: &[&str], feedback: &Feedback) -> String {
    let mut out = general_condition_xml(feedback);
    let value = even_share_score(matching.pairs.len());
    for (index, pair) in matching.pairs.iter().enumerate() {
        let correct = options
            .iter()
            .position(|right| *right == pair.right)
            .unwrap_or(0);
        out.push_str(
            &ADD_CONDITION_TEMPLATE
                .replace("{{RESPIDENT}}", &match_response_ident(index))
                .replace("{{IDENT}}", &match_option_ident(correct))
                .replace("{{VALUE}}", &value),
        );
    }
    out
}

/// Render an ordering item: draggable options scored only when fully in order.
fn ordering_item_xml(question: &Question, ordering: &Ordering) -> String {
    let (options, answer_ids) = order_options_xml(ordering);
    fill_item_header(ORDERING_ITEM_TEMPLATE, question)
        .replace("{{ANSWER_IDS}}", &answer_ids)
        .replace("{{OPTIONS}}", &options)
        .replace(
            "{{CONDITIONS}}",
            &order_conditions_xml(ordering, &question.feedback),
        )
        .replace(
            "{{ITEMFEEDBACK}}",
            &general_itemfeedback_xml(&question.feedback),
        )
        // `{{PROMPT}}` is filled last so authored text is never re-scanned.
        .replace("{{PROMPT}}", &escaped_html(&question.prompt))
}

/// The response identifier for the ordering option authored at `index`.
fn order_answer_ident(index: usize) -> String {
    format!("answer_{index}")
}

/// Render the draggable options in display order, returning the `response_label`
/// XML and the matching comma-separated `original_answer_ids` in that order.
fn order_options_xml(ordering: &Ordering) -> (String, String) {
    let mut options = String::new();
    let mut ids: Vec<String> = Vec::new();
    for index in ordering.display_order() {
        let ident = order_answer_ident(index);
        let text = ordering.items.get(index).map_or("", String::as_str);
        options.push_str(
            &ORDER_OPTION_TEMPLATE
                .replace("{{OPTION_IDENT}}", &ident)
                .replace("{{TEXT}}", &escaped_html(text)),
        );
        ids.push(ident);
    }
    (options, ids.join(","))
}

/// Render the scoring conditions: any general feedback, then the single
/// all-or-nothing condition listing every option in authored (correct) order.
fn order_conditions_xml(ordering: &Ordering, feedback: &Feedback) -> String {
    let mut varequals = String::new();
    for index in 0..ordering.items.len() {
        varequals.push_str(
            &ORDER_VAREQUAL_TEMPLATE.replace("{{OPTION_IDENT}}", &order_answer_ident(index)),
        );
    }
    let condition = ORDER_CONDITION_TEMPLATE.replace("{{VAREQUALS}}", &varequals);
    format!("{}{condition}", general_condition_xml(feedback))
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
    let resprocessing = resprocessing_xml(correct_cv, incorrect_cv, feedback);
    fill_choice_item(
        template,
        question,
        &resprocessing,
        &itemfeedback_xml(feedback),
    )
}

/// Fill the shared choice-item body: resprocessing, itemfeedback, then prompt.
///
/// `{{PROMPT}}` is substituted last so authored text is never re-scanned.
fn fill_choice_item(
    template: &str,
    question: &Question,
    resprocessing: &str,
    itemfeedback: &str,
) -> String {
    fill_item_header(template, question)
        .replace("{{RESPROCESSING}}", resprocessing)
        .replace("{{ITEMFEEDBACK}}", itemfeedback)
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
///
/// Local image references are rewritten to Canvas's `$IMS-CC-FILEBASE$` form so
/// they resolve to the files bundled under `web_resources/`; absolute URLs
/// (`http(s)://`, `//`, `data:`, root-relative) are left untouched. Inline and
/// display math (`$…$` / `$$…$$`) become Canvas's native `equation_image`.
///
/// GitHub-flavored extensions are enabled so common authoring — pipe tables and
/// `~~strikethrough~~` — renders as real HTML rather than literal text.
fn prompt_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH;
    let mut rendered = String::new();
    let parser = Parser::new_ext(markdown, options)
        .map(rewrite_local_image)
        .map(rewrite_math);
    html::push_html(&mut rendered, parser);
    rendered.trim().to_owned()
}

/// Rewrite a local image event's URL to its bundled `$IMS-CC-FILEBASE$` form.
fn rewrite_local_image(event: Event) -> Event {
    match event {
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) if is_local_image(&dest_url) => Event::Start(Tag::Image {
            link_type,
            dest_url: canvas_image_ref(&dest_url).into(),
            title,
            id,
        }),
        other => other,
    }
}

/// Rewrite an inline/display math event to Canvas's native `equation_image` HTML.
///
/// pulldown would otherwise render math as a `<span class="math">`; Canvas
/// instead renders LaTeX from its equation service, so the raw event is replaced
/// with the matching `<img>`.
fn rewrite_math(event: Event) -> Event {
    match event {
        Event::InlineMath(latex) | Event::DisplayMath(latex) => {
            Event::InlineHtml(equation_image_html(&latex).into())
        }
        other => other,
    }
}

/// Canvas's native equation image; `{{ENCODED}}` is the double-percent-encoded
/// LaTeX for the `src`, `{{ATTR}}` the entity-escaped LaTeX for the text
/// attributes (escaped once here, then again with the surrounding prompt HTML).
const EQUATION_IMAGE_TEMPLATE: &str = "<img class=\"equation_image\" \
     src=\"/equation_images/{{ENCODED}}\" alt=\"LaTeX: {{ATTR}}\" \
     data-equation-content=\"{{ATTR}}\" title=\"{{ATTR}}\">";

/// Canvas's native equation image for `latex`, rendered by its equation service.
///
/// The `src` double-percent-encodes the LaTeX (Canvas decodes it twice) and is
/// host-relative so it resolves on whatever Canvas instance imports the bank.
/// The raw LaTeX travels in `data-equation-content` for Canvas to re-render.
fn equation_image_html(latex: &str) -> String {
    EQUATION_IMAGE_TEMPLATE
        .replace(
            "{{ENCODED}}",
            &percent_encode_component(&percent_encode_component(latex)),
        )
        .replace("{{ATTR}}", &escape_xml(latex))
}

/// The `$IMS-CC-FILEBASE$` reference for a bundled local image `path`.
fn canvas_image_ref(path: &str) -> String {
    format!(
        "$IMS-CC-FILEBASE$/{}?canvas_download=1",
        percent_encode_path(path)
    )
}

/// Percent-encode a path for a URL, keeping `/` and the unreserved characters.
fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Percent-encode `text` as a single URL component: every byte except the
/// unreserved set (alphanumerics and `-_.~`) becomes `%XX`, including `/`.
fn percent_encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            // Writing to a `String` is infallible, so the result is discarded.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The distinct local-image paths referenced by any question in `bank`.
///
/// Scans every rich-text field (see `Question::rich_text_fields`), so images
/// embedded in answers and feedback are bundled just like prompt images. The
/// exporter bundles these under `web_resources/`; the CLI resolves them to bytes
/// relative to the question directory.
#[must_use]
pub fn local_image_paths(bank: &ItemBank) -> Vec<String> {
    let mut paths = Vec::new();
    for question in &bank.items {
        for field in question.rich_text_fields() {
            for url in image_urls(field) {
                if is_local_image(&url) && !paths.contains(&url) {
                    paths.push(url);
                }
            }
        }
    }
    paths
}

/// The image URLs referenced by a Markdown `prompt`, in order.
fn image_urls(prompt: &str) -> Vec<String> {
    Parser::new(prompt)
        .filter_map(|event| match event {
            Event::Start(Tag::Image { dest_url, .. }) => Some(dest_url.to_string()),
            _ => None,
        })
        .collect()
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

/// Zip the `(path, bytes)` files into a package byte payload.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] if the zip archive cannot be written.
fn zip_package(files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default();
        for (path, content) in files {
            writer.start_file(*path, options).map_err(|e| zip_err(&e))?;
            writer.write_all(content)?;
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
                kind: QuestionKind::MultipleSelect(MultipleSelect {
                    scoring: ScoringMode::AllOrNothing,
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
    fn multiple_select_bank(
        choices: &[(&str, bool)],
        feedback: Feedback,
        scoring: ScoringMode,
    ) -> ItemBank {
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
                kind: QuestionKind::MultipleSelect(MultipleSelect { choices, scoring }),
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
        let bank = multiple_select_bank(
            &[("A", true), ("B", false), ("C", true)],
            feedback,
            ScoringMode::AllOrNothing,
        );
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("<not>"));
        assert!(xml.contains("<and>"));
        assert!(xml.contains(r#"linkrefid="incorrect_fb""#));
        assert!(xml.contains(r#"<itemfeedback ident="correct_fb">"#));
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
    }

    #[test]
    /// Partial-credit scoring adds an even share per correct choice and
    /// subtracts per incorrect one, with no all-or-nothing `<and>` block.
    fn multiple_select_partial_credit_adds_and_subtracts() {
        let bank = multiple_select_bank(
            &[("A", true), ("B", false), ("C", true)],
            Feedback::default(),
            ScoringMode::Partial,
        );
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("multiple_answers_question"));
        assert!(!xml.contains("<and>"));
        // Two correct → +50.00 each; one incorrect → -100.00.
        assert!(xml.contains(r#"<setvar action="Add" varname="SCORE">50.00</setvar>"#));
        assert!(xml.contains(r#"<setvar action="Subtract" varname="SCORE">100.00</setvar>"#));
        assert!(xml.contains(r#"<varequal respident="response1">choice_1</varequal>"#));
    }

    #[test]
    /// Partial credit wires general feedback only (no correct/incorrect arms).
    fn multiple_select_partial_wires_general_feedback_only() {
        let feedback = Feedback {
            general: Some("Recall the set.".to_owned()),
            correct: Some("Yes.".to_owned()),
            incorrect: Some("No.".to_owned()),
        };
        let bank = multiple_select_bank(
            &[("A", true), ("B", false), ("C", true)],
            feedback,
            ScoringMode::Partial,
        );
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
        assert!(xml.contains(r#"linkrefid="general_fb""#));
        // Correct/incorrect feedback has no home under partial scoring.
        assert!(!xml.contains(r#"ident="correct_fb""#));
        assert!(!xml.contains(r#"ident="incorrect_fb""#));
    }

    #[test]
    /// An all-correct partial set adds credit but emits no `Subtract` arm.
    fn multiple_select_partial_all_correct_has_no_subtract() {
        let bank = multiple_select_bank(
            &[("A", true), ("B", true)],
            Feedback::default(),
            ScoringMode::Partial,
        );
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(r#"<setvar action="Add" varname="SCORE">50.00</setvar>"#));
        assert!(!xml.contains(r#"action="Subtract""#));
        assert!(!xml.contains("<and>"));
    }

    #[test]
    /// An all-correct multiple-select emits only `<varequal>` terms, no `<not>`.
    fn multiple_select_all_correct_has_no_not_terms() {
        let bank = multiple_select_bank(
            &[("A", true), ("B", true)],
            Feedback::default(),
            ScoringMode::AllOrNothing,
        );
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
    fn even_share_score_splits_evenly() {
        assert_eq!(even_share_score(1), "100.00");
        assert_eq!(even_share_score(2), "50.00");
        assert_eq!(even_share_score(3), "33.33");
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
    /// Matching renders one response per left, shared options, and per-pair
    /// partial-credit scoring pairing each left with its correct option.
    fn matching_renders_responses_and_scoring() {
        let pair = |left: &str, right: &str| MatchPair {
            left: left.to_owned(),
            right: right.to_owned(),
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "mt".to_owned(),
                title: None,
                prompt: "Match.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::Matching(Matching {
                    pairs: vec![pair("char", "1 byte"), pair("int", "4 bytes")],
                    distractors: vec!["8 bytes".to_owned()],
                }),
            }],
        };
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("matching_question"));
        // One response per left prompt, each offering all three options.
        assert!(xml.contains(r#"<response_lid ident="response_0">"#));
        assert!(xml.contains(r#"<response_lid ident="response_1">"#));
        assert!(xml.contains("char") && xml.contains("8 bytes"));
        // Two pairs -> 50.00 each; left 1 (int -> 4 bytes) pairs with option 1.
        assert!(xml.contains(r#"<setvar action="Add" varname="SCORE">50.00</setvar>"#));
        assert!(xml.contains(r#"<varequal respident="response_0">answer_0</varequal>"#));
        assert!(xml.contains(r#"<varequal respident="response_1">answer_1</varequal>"#));
    }

    #[test]
    /// A right reused across pairs (or shared with a distractor) collapses to one
    /// option, so a later pair scores against an earlier, non-identity option.
    fn matching_dedups_options_and_pairs_by_value() {
        let pair = |left: &str, right: &str| MatchPair {
            left: left.to_owned(),
            right: right.to_owned(),
        };
        let matching = Matching {
            // "cat" repeats and the distractor duplicates "dog".
            pairs: vec![pair("a", "cat"), pair("b", "dog"), pair("c", "cat")],
            distractors: vec!["dog".to_owned()],
        };
        let bank = matching_bank(matching, Feedback::default());
        let xml = assessment_xml("a", &bank).expect("renders");
        // Two distinct options only: answer_0/answer_1, never answer_2.
        assert!(xml.contains("answer_1") && !xml.contains("answer_2"));
        // Three pairs -> 33.33 each; pair "c" maps back to option 0, not 2.
        assert!(xml.contains(r#"<setvar action="Add" varname="SCORE">33.33</setvar>"#));
        assert!(xml.contains(r#"<varequal respident="response_2">answer_0</varequal>"#));
    }

    #[test]
    /// Matching wires general feedback and XML-escapes left/right cell text.
    fn matching_wires_feedback_and_escapes_cells() {
        let pair = |left: &str, right: &str| MatchPair {
            left: left.to_owned(),
            right: right.to_owned(),
        };
        let matching = Matching {
            pairs: vec![pair("a < b", "x & y"), pair("c", "z")],
            distractors: Vec::new(),
        };
        let feedback = Feedback {
            general: Some("Recall the ordering.".to_owned()),
            correct: Some("nice".to_owned()),
            incorrect: Some("nope".to_owned()),
        };
        let xml = assessment_xml("a", &matching_bank(matching, feedback)).expect("renders");
        assert!(xml.contains("a &lt; b") && xml.contains("x &amp; y"));
        assert!(!xml.contains("a < b") && !xml.contains("x & y"));
        // General feedback is wired; answer-level feedback is not (matching-only).
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
        assert!(!xml.contains(r#"ident="correct_fb""#) && !xml.contains(r#"ident="incorrect_fb""#));
    }

    /// A one-item bank wrapping `matching` with the given `feedback`.
    fn matching_bank(matching: Matching, feedback: Feedback) -> ItemBank {
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "mt".to_owned(),
                title: None,
                prompt: "Match.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback,
                kind: QuestionKind::Matching(matching),
            }],
        }
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
        let bytes = to_qti(&multiple_choice_bank(0, Feedback::default()), &[]).expect("export");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        assert_eq!(archive.len(), 2);
    }

    #[test]
    /// The package is a zip holding the manifest and one assessment file.
    fn package_is_zip_with_manifest_and_assessment() {
        let bytes = to_qti(&true_false_bank(true, "Q?"), &[]).expect("export succeeds");
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
    /// A Markdown pipe table renders as an HTML `<table>`, not literal text.
    fn markdown_table_renders_as_html() {
        let prompt = "Costs:\n\n| Op | Cost |\n|----|------|\n| push | O(1) |\n";
        let xml = assessment_xml("a", &true_false_bank(true, prompt)).expect("renders");
        // mattext is HTML-escaped, so `<table>` appears as `&lt;table&gt;`.
        assert!(xml.contains("&lt;table&gt;"));
        assert!(xml.contains("&lt;td&gt;push&lt;/td&gt;"));
        assert!(!xml.contains("| push |"));
    }

    #[test]
    /// GFM `~~strikethrough~~` renders as `<del>`.
    fn markdown_strikethrough_renders() {
        let xml =
            assessment_xml("a", &true_false_bank(true, "This is ~~wrong~~.")).expect("renders");
        assert!(xml.contains("&lt;del&gt;wrong&lt;/del&gt;"));
    }

    #[test]
    /// Inline `$…$` math becomes Canvas's native equation image (double-encoded).
    fn inline_math_becomes_equation_image() {
        let xml =
            assessment_xml("a", &true_false_bank(true, "It is $x^2$ hard.")).expect("renders");
        assert!(xml.contains("equation_image"));
        // `^` is percent-encoded twice: `%5E` -> `%255E`.
        assert!(xml.contains("/equation_images/x%255E2"));
        assert!(xml.contains("data-equation-content=&quot;x^2&quot;"));
    }

    #[test]
    /// Display `$$…$$` math also renders as an equation image.
    fn display_math_becomes_equation_image() {
        let xml = assessment_xml("a", &true_false_bank(true, "$$E=mc^2$$")).expect("renders");
        assert!(xml.contains("equation_image"));
        assert!(xml.contains("data-equation-content=&quot;E=mc^2&quot;"));
    }

    #[test]
    /// LaTeX specials are HTML- then XML-escaped (double-encoded) in attributes.
    fn math_escapes_special_characters() {
        let xml = assessment_xml("a", &true_false_bank(true, "$a < b$")).expect("renders");
        assert!(xml.contains("a &amp;lt; b"));
        assert!(!xml.contains("a < b"));
    }

    #[test]
    /// A lone `$` with no closing delimiter stays literal, not an equation.
    fn lone_dollar_is_not_math() {
        let xml =
            assessment_xml("a", &true_false_bank(true, "It costs $5 today.")).expect("renders");
        assert!(!xml.contains("equation_image"));
    }

    #[test]
    /// The double-encoded `src` matches Canvas's own export for `O(\log n)`.
    fn math_src_matches_canvas_double_encoding() {
        let xml =
            assessment_xml("a", &true_false_bank(true, r"Cost $O(\log n)$.")).expect("renders");
        assert!(xml.contains("/equation_images/O%2528%255Clog%2520n%2529"));
    }

    #[test]
    /// A double-quote in LaTeX is entity-escaped so it cannot break the attribute.
    fn math_escapes_quotes_in_latex() {
        let xml = assessment_xml("a", &true_false_bank(true, r#"$\text{"x"}$"#)).expect("renders");
        // `"` -> `&quot;` (here) -> `&amp;quot;` (outer XML escape).
        assert!(xml.contains("&amp;quot;x&amp;quot;"));
    }

    #[test]
    /// Math renders inside a choice, not just a prompt (both HTML paths).
    fn math_renders_in_a_choice() {
        let bank = choice_image_bank(vec![("$x^2$", true), ("No", false)], Feedback::default());
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("equation_image"));
        assert!(xml.contains("data-equation-content=&quot;x^2&quot;"));
    }

    #[test]
    /// The manifest wires the resource ident and href into the resource nodes.
    fn manifest_references_assessment_resource() {
        let xml = manifest_xml("assessment_m", "assessment_m/assessment_m.xml", &[]);
        assert!(xml.contains(r#"identifier="assessment_m""#));
        assert!(xml.contains(r#"href="assessment_m/assessment_m.xml""#));
    }

    /// Build a one-question bank whose prompt is `prompt`.
    fn image_bank(prompt: &str) -> ItemBank {
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: prompt.to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        }
    }

    #[test]
    /// A local image URL is rewritten to `$IMS-CC-FILEBASE$`; external URLs stay.
    fn local_image_rewritten_external_untouched() {
        let bank = image_bank("See ![d](diagram.png) and ![x](https://ex.com/x.png).");
        assert_eq!(local_image_paths(&bank), ["diagram.png"]);
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("$IMS-CC-FILEBASE$/diagram.png?canvas_download=1"));
        assert!(xml.contains("https://ex.com/x.png"));
        // The bare local src must not survive.
        assert!(!xml.contains("src=&quot;diagram.png&quot;"));
    }

    #[test]
    /// Supplied image bytes are bundled and declared in the manifest.
    fn images_are_bundled_and_declared() {
        let bank = image_bank("![d](sub/diagram.png)");
        let images = vec![("sub/diagram.png".to_owned(), vec![1_u8, 2, 3])];
        let bytes = to_qti(&bank, &images).expect("export");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        let names: Vec<&str> = archive.file_names().collect();
        assert!(names.contains(&"web_resources/sub/diagram.png"));
        let manifest = manifest_xml("a", "a/a.xml", &images);
        assert!(manifest.contains(r#"href="web_resources/sub/diagram.png""#));
        assert!(manifest.contains("webcontent"));
    }

    /// A one-question multiple-choice bank over `choices`, with `feedback`.
    fn choice_image_bank(choices: Vec<(&str, bool)>, feedback: Feedback) -> ItemBank {
        let choices = choices
            .into_iter()
            .map(|(text, correct)| Choice {
                text: text.to_owned(),
                correct,
            })
            .collect();
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
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
    /// Local images in choices and feedback are collected; externals are not.
    fn local_image_paths_scans_answers_and_feedback() {
        let feedback = Feedback {
            general: Some("Recall balance: ![hint](hint.png)".to_owned()),
            correct: None,
            incorrect: None,
        };
        let choices = vec![
            ("![a](pic.png)", true),
            ("![b](https://ex.com/y.png)", false),
        ];
        let bank = choice_image_bank(choices, feedback);
        // Choice image and feedback image collected, in field order; external skipped.
        assert_eq!(local_image_paths(&bank), ["pic.png", "hint.png"]);
    }

    #[test]
    /// A local image referenced only in an answer is bundled into the package.
    fn answer_image_is_bundled() {
        let bank = choice_image_bank(
            vec![("![a](tree.png)", true), ("Neither", false)],
            Feedback::default(),
        );
        assert_eq!(local_image_paths(&bank), ["tree.png"]);
        let images = vec![("tree.png".to_owned(), vec![9_u8, 9, 9])];
        let bytes = to_qti(&bank, &images).expect("export");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        assert!(
            archive
                .file_names()
                .any(|name| name == "web_resources/tree.png")
        );
    }

    #[test]
    /// A local image embedded in an ordering item is collected for bundling.
    fn ordering_item_image_is_collected() {
        let items = vec!["![step](step.png)".to_owned(), "Plain".to_owned()];
        let bank = ordering_bank(items, Feedback::default());
        assert_eq!(local_image_paths(&bank), ["step.png"]);
    }

    #[test]
    /// The same local image in the prompt and a choice is bundled once.
    fn duplicate_image_across_fields_dedups() {
        let mut bank = choice_image_bank(
            vec![("![a](same.png)", true), ("No", false)],
            Feedback::default(),
        );
        if let Some(question) = bank.items.first_mut() {
            question.prompt = "See ![p](same.png)".to_owned();
        }
        assert_eq!(local_image_paths(&bank), ["same.png"]);
    }

    #[test]
    /// Matching cells are plain text, so an image in one is not collected.
    fn matching_cell_image_not_collected() {
        let matching = Matching {
            pairs: vec![
                MatchPair {
                    left: "![x](m.png)".to_owned(),
                    right: "one".to_owned(),
                },
                MatchPair {
                    left: "b".to_owned(),
                    right: "two".to_owned(),
                },
            ],
            distractors: Vec::new(),
        };
        assert!(local_image_paths(&matching_bank(matching, Feedback::default())).is_empty());
    }

    #[test]
    /// A generated-diagram image bundles like any other (the exporter is
    /// provenance-agnostic; the CLI supplies the bytes from the diagram pass).
    fn generated_diagram_image_is_bundled() {
        let bank = image_bank("![d](generated/mermaid-abc.png)");
        let images = vec![("generated/mermaid-abc.png".to_owned(), vec![1_u8, 2, 3])];
        let bytes = to_qti(&bank, &images).expect("export");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        assert!(
            archive
                .file_names()
                .any(|name| name == "web_resources/generated/mermaid-abc.png")
        );
    }

    #[test]
    /// A Graphviz block travels the whole Canvas path: the diagram pass replaces
    /// the fence, the item HTML points at the generated image, and its bytes are
    /// bundled under `web_resources/`.
    fn graphviz_diagram_reaches_canvas_package() {
        use crate::diagram::{DiagramFormat, DiagramLanguage, render_diagrams};
        let mut bank = image_bank("```dot\ndigraph { a -> b; }\n```");
        let render = |_language: DiagramLanguage, _source: &str| Ok(vec![1_u8, 2, 3]);
        let outcome = render_diagrams(&mut bank, &render, DiagramFormat::Png);
        let path = outcome
            .images
            .first()
            .map(|(path, _)| path.clone())
            .expect("one generated image");
        assert!(path.starts_with("generated/graphviz-"));
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains(&format!("$IMS-CC-FILEBASE$/{path}?canvas_download=1")));
        let bytes = to_qti(&bank, &outcome.images).expect("export");
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("valid zip");
        let bundled = format!("web_resources/{path}");
        assert!(archive.file_names().any(|name| name == bundled));
    }

    #[test]
    /// An unrendered Graphviz block still exports: the fence reaches Canvas as a
    /// code block instead of an image, and nothing is bundled for it.
    fn unrendered_graphviz_block_exports_as_code() {
        let bank = image_bank("```dot\ndigraph { a -> b; }\n```");
        let xml = assessment_xml("a", &bank).expect("renders");
        assert!(xml.contains("language-dot") && xml.contains("digraph"));
        assert!(local_image_paths(&bank).is_empty());
    }

    #[test]
    /// Path percent-encoding keeps `/` but escapes spaces.
    fn percent_encode_keeps_slash_escapes_space() {
        assert_eq!(percent_encode_path("a b/c.png"), "a%20b/c.png");
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
    /// Ordering shuffles options for display (sorted, so not the answer) yet
    /// scores the single correct sequence in authored order, all-or-nothing.
    fn ordering_renders_shuffled_options_and_correct_order() {
        // Authored (correct) order; sorted display is Alloc, Free, Init, Use.
        let items = ["Alloc", "Init", "Use", "Free"].map(str::to_owned).to_vec();
        let xml = assessment_xml("a", &ordering_bank(items, Feedback::default())).expect("renders");
        assert!(xml.contains("ordering_question"));
        assert!(xml.contains(r#"<response_lid ident="response1" rcardinality="Ordered">"#));
        // Display order is sorted by text: Alloc(0), Free(3), Init(1), Use(2).
        assert!(xml.contains("<fieldentry>answer_0,answer_3,answer_1,answer_2</fieldentry>"));
        // The scoring condition lists options in authored (correct) order.
        assert!(xml.contains(concat!(
            "<varequal respident=\"response1\">answer_0</varequal>\n",
            "              <varequal respident=\"response1\">answer_1</varequal>\n",
            "              <varequal respident=\"response1\">answer_2</varequal>\n",
            "              <varequal respident=\"response1\">answer_3</varequal>"
        )));
        assert!(xml.contains(r#"<setvar action="Set" varname="SCORE">100</setvar>"#));
    }

    #[test]
    /// Ordering wires general feedback and XML-escapes option text.
    fn ordering_wires_feedback_and_escapes() {
        let items = vec!["a < b".to_owned(), "c & d".to_owned()];
        let feedback = Feedback {
            general: Some("Think about precedence.".to_owned()),
            correct: Some("yes".to_owned()),
            incorrect: Some("no".to_owned()),
        };
        let xml = assessment_xml("a", &ordering_bank(items, feedback)).expect("renders");
        assert!(xml.contains("a &amp;lt; b") && xml.contains("c &amp;amp; d"));
        assert!(!xml.contains("a < b") && !xml.contains("c & d"));
        // General feedback is wired; answer-level feedback is not.
        assert!(xml.contains(r#"<itemfeedback ident="general_fb">"#));
        assert!(!xml.contains(r#"ident="correct_fb""#) && !xml.contains(r#"ident="incorrect_fb""#));
    }

    /// A one-item bank wrapping an ordering question over `items` (correct order).
    fn ordering_bank(items: Vec<String>, feedback: Feedback) -> ItemBank {
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "ord".to_owned(),
                title: None,
                prompt: "Order.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback,
                kind: QuestionKind::Ordering(Ordering { items }),
            }],
        }
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
