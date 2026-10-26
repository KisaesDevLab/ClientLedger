//! English text for the optional readers (A03 WP-G-cloud, step 21): Ask AI
//! to help on Enter Bill (`bill.read.assist.*`) and Preferences › Reading
//! documents online (`prefs.cloud_reading.*`). Only this work package adds
//! rows.

/// The optional readers' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // A03 WP-G-cloud: Ask AI to help (Enter Bill).
    ("bill.read.assist.button", "Ask AI to help"),
    ("bill.read.assist.button.tip.local", "Asks your AI on this computer (Ollama) to fill what couldn't be read. Nothing leaves this computer. You check every value before saving."),
    ("bill.read.assist.button.tip.cloud", "Sends the document's text to your online AI service to fill what couldn't be read. You check every value before saving."),
    ("bill.read.assist.reading", "Asking AI to help\u{2026}"),
    ("bill.read.assist.consent", "Your AI service is online, so it can help read documents only if you allow it in Preferences \u{203a} Reading documents online."),
    ("bill.read.assist.helped", "AI helped with this reading. An amount it suggested is used only when the same amount is printed on the document."),
    ("bill.read.assist.problem.consent", "This company hasn't agreed to send documents to an online AI service. Allow it in Preferences \u{203a} Reading documents online, or type the values in."),
    ("bill.read.assist.problem.pictures", "Your AI settings don't allow pictures to be sent. Type the values in yourself."),
    // A03 WP-G-cloud: Preferences › Reading documents online.
    ("prefs.cat.cloud_reading", "Reading documents online"),
    ("prefs.cloud_reading.title", "Reading documents with an online service"),
    ("prefs.cloud_reading.intro", "ClientLedger reads your bills and receipts here on this computer. When it can't read something clearly, you can choose Ask AI to help on the bill. These switches decide whether that help may send the document to your online AI service (Anthropic or OpenAI). Both are off until you turn them on."),
    ("prefs.cloud_reading.local", "If your AI runs on this computer (Ollama), nothing leaves it and these switches aren't needed."),
    ("prefs.cloud_reading.what", "What may be sent"),
    ("prefs.cloud_reading.text", "Send the document's text"),
    ("prefs.cloud_reading.text_desc", "The words ClientLedger read from the page, when you choose Ask AI to help. Personal details your AI settings recognise, such as account and card numbers, are hidden first."),
    ("prefs.cloud_reading.images", "Also send pictures of the page"),
    ("prefs.cloud_reading.images_desc", "Only for a document that is itself a picture (PNG). A picture can help with a hard-to-read scan."),
    ("prefs.cloud_reading.images_warning", "A picture can't have anything hidden: everything printed on it, including names, addresses and account numbers, goes to the AI service as it is."),
    ("prefs.cloud_reading.images_blocked", "Your AI settings don't allow pictures to be sent, so only the text is sent for now."),
    ("prefs.cloud_reading.amounts", "AI never sets an amount on its own: an amount it suggests is used only when the same amount is printed on the page, and you accept every value before saving."),
    ("prefs.cloud_reading.audit", "Each change to these switches is recorded in the company's audit trail."),
    ("prefs.cloud_reading.needs_engine", "This needs a newer version of the bookkeeping engine. Until then, nothing is sent online."),
    ("prefs.cloud_reading.not_owner", "Only the company's owner can change these switches."),
    ("prefs.cloud_reading.ai_off", "AI suggestions are off for this company. Turn them on first in AI suggestions."),
    ("prefs.cloud_reading.reset", "Saved. Because AI was agreed to again (or turned off), sending documents online is off again. Turn it back on here if you want it."),
    ("prefs.cloud_reading.link_hint", "Bills and receipts go to this service only if you allow it in"),
    ("prefs.cloud_reading.link", "Reading documents online"),
    ("prefs.cloud_reading.link_tip", "Choose whether documents may go to your AI service when you ask AI to help read them."),
];
