use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
    base::{CFTypeRef, TCFType},
    set::{CFSet, CFSetGetCount, CFSetGetValues, CFSetRef},
    string::{CFString, CFStringRef},
};
use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSDocumentTypeDocumentAttribute, NSHTMLTextDocumentType,
    NSMutableAttributedStringDocumentFormats, NSPasteboard, NSPasteboardTypeString,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSMutableAttributedString, NSString, NSUTF8StringEncoding,
};
use std::{collections::HashMap, ffi::c_void};

#[repr(C)]
pub struct __DCSDictionary(c_void);
pub type DCSDictionaryRef = *const __DCSDictionary;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    pub fn DCSCopyAvailableDictionaries() -> CFSetRef;
    pub fn DCSCopyRecordsForSearchString(
        dictionary: DCSDictionaryRef,
        string: CFStringRef,
        u1: *mut c_void, // Usually NULL
        u2: *mut c_void, // Usually NULL
    ) -> CFArrayRef;
    pub fn DCSDictionaryGetName(dict: DCSDictionaryRef) -> CFStringRef;
    pub fn DCSRecordCopyData(record: CFTypeRef) -> CFStringRef;
}

pub fn get_dictionary_attributed_string(
    word: &str,
    dict_names: &[String],
) -> Option<Retained<NSMutableAttributedString>> {
    autoreleasepool(|_| {
        unsafe {
            let dicts_set_ref = DCSCopyAvailableDictionaries();
            if dicts_set_ref.is_null() {
                return None;
            }
            let dicts: CFSet<DCSDictionaryRef> = CFSet::wrap_under_create_rule(dicts_set_ref);

            // Allocate a buffer to hold the pointers
            let count = CFSetGetCount(dicts.as_concrete_TypeRef());
            let mut values: Vec<*const c_void> = vec![std::ptr::null(); count as usize];
            CFSetGetValues(dicts.as_concrete_TypeRef(), values.as_mut_ptr());

            let mut dictionaries = HashMap::new();
            let cfstring = CFString::new(word);

            for dict_ptr in values {
                if dict_ptr.is_null() {
                    continue;
                }
                let dict_ptr = dict_ptr as DCSDictionaryRef;

                // Get dictionary name
                let name_ref = DCSDictionaryGetName(dict_ptr);
                if name_ref.is_null() {
                    continue;
                }
                let name_cfstr = CFString::wrap_under_get_rule(name_ref);
                dictionaries.insert(name_cfstr.to_string(), dict_ptr);
            }

            // Respect the dict order preference
            for dict_name in dict_names {
                let Some(dict_ptr) = dictionaries.get(dict_name) else {
                    continue;
                };
                if dict_ptr.is_null() {
                    continue;
                }
                let records_ptr = DCSCopyRecordsForSearchString(
                    *dict_ptr,
                    cfstring.as_concrete_TypeRef(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );

                if records_ptr.is_null() {
                    continue;
                }

                // Get the first entry
                let record: CFArray<AnyObject> = CFArray::wrap_under_create_rule(records_ptr);
                let count = CFArrayGetCount(record.as_concrete_TypeRef());
                if count == 0 {
                    continue;
                }
                let first = CFArrayGetValueAtIndex(record.as_concrete_TypeRef(), 0);
                if first.is_null() {
                    continue;
                }
                let first_record = CFTypeRef::from(first);

                let html_ptr = DCSRecordCopyData(first_record);
                if !html_ptr.is_null() {
                    let html = CFString::wrap_under_create_rule(html_ptr);
                    // println!("{html:?}");
                    return html_to_attributed_string(&html.to_string());
                }
            }
        }
        None
    })
}

/// Converts an HTML string into an NSMutableAttributedString, applying bold/italic
/// styles to specific classes via CSS injection.
pub fn html_to_attributed_string(html: &str) -> Option<Retained<NSMutableAttributedString>> {
    // NOTE: Define the CSS style block mapping your classes to font styles.
    // TODO: Indentation, might require regex replacing
    let style = r#"
<style>
/* --- Global Reset & Typography --- */
body {
    font-size: 15px;
    line-height: 1.5;
}

/* --- Block Layout (The "Newline" Logic) --- */
/* These classes represent major sections that should start on a new line */
.hwg, .hg,          /* Headword groups */
.semb, .gramb, .se1,       /* Grammar/Sense groups */
.msDict,            /* Main dictionary definitions */
.exg, .eg,          /* Example groups */
.subEntryBlock,     /* Derivatives section */
.etym,              /* Etymology section */
d\:entry {          /* The root entry tag */
    display: block;
}

/* --- Headword Styling --- */
.hw {
    font-family: sans-serif;
    font-weight: bold;
    font-size: 2.0em;
}

/* Phonetics */
.ph {
    font-family: monospace;
}

/* --- Definitions & Part of Speech --- */
.ps, .pos {
    font-style: italic;
}

.df, .trans {
    font-family: sans-serif;
}
</style>"#;

    // HACK: for malformed html, cleanup the <head>
    let clean_html = html.replace("<head/>", "").replace("</head>", "");
    let processed_html = if clean_html.contains("<body>") {
        clean_html.replace("<body>", &format!("<body>{}", style))
    } else {
        format!("<html><body>{}{}</body></html>", style, clean_html)
    };

    let ns_html = NSString::from_str(&processed_html);
    let data = ns_html.dataUsingEncoding(NSUTF8StringEncoding)?;

    unsafe {
        let values: [Retained<AnyObject>; 1] =
            [NSString::from_str(&NSHTMLTextDocumentType.to_string()).into()];
        let options =
            NSDictionary::from_retained_objects(&[NSDocumentTypeDocumentAttribute], &values);

        let attr_str = NSMutableAttributedString::new();
        attr_str
            .readFromData_options_documentAttributes_error(&data, &options, None)
            .ok()?;

        Some(attr_str)
    }
}

pub fn text_to_clipboard(text: &str) {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns_string = NSString::from_str(text);
        let proto_string = ProtocolObject::from_retained(ns_string);
        let objects = NSArray::from_retained_slice(&[proto_string]);
        pb.writeObjects(&objects);
    })
}

pub fn text_from_clipboard() -> Option<String> {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.stringForType(unsafe { NSPasteboardTypeString })
            .map(|rnss| rnss.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICTIONARY_HTML: &str = r#"
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">
<html xmlns:d="http://www.apple.com/DTDs/DictionaryService-1.0.rng">
<head><meta http-equiv="Content-Type" content="text/html; charset=UTF-8" /></head>
<body><d:entry id="e_b-en-zh_hans0054435" d:title="notebook" class="entry" lang="zh-cmn-Hans" xml:lang="zh-cmn-Hans">
<span class="hwg x_xh0"><span d:dhw="1" role="text" class="hw">notebook </span>
<span prxid="notebook_gb_5b16" prlexid="optra0083961.001" dialect="BrE" class="prx">
<span class="gp tg_prx"> | </span><span class="gp tg_prx">BrE </span><span d:prn="UK_IPA solitary" soundFile="notebook#_gb_1" dialect="BrE" class="ph">ˈnəʊtbʊk<d:prn></d:prn></span>
<span class="gp tg_prx">, </span></span><span prxid="notebook_us_5b1c" prlexid="optra0083961.002" dialect="AmE" class="prx"><span class="gp tg_prx">AmE </span>
<span d:prn="UK_IPA solitary" soundFile="notebook#_us_1" dialect="AmE" class="ph">ˈnoʊtˌbʊk<d:prn></d:prn></span>
<span class="gp tg_prx"> | </span></span><span class="gp tg_hwg"> </span></span><span lexid="b-en-zh_hans0054435.001" class="gramb x_xd0"><span class="x_xdh">
<span d:pos="1" class="ps">noun <d:pos></d:pos></span><span class="gp">  </span> </span>
<span lexid="b-en-zh_hans0054435.002" class="semb x_xd1 hasSn"><span class="gp x_xdh sn ty_label tg_semb">① </span>
<span class="trg x_xd2"><span class="ind"><span class="gp tg_ind">(</span>small book<span class="gp tg_ind">) </span></span>
<span d:def="1" class="trans">笔记本 <d:def></d:def></span>
<span class="trans ty_pinyin">bǐjìběn </span></span></span><span lexid="b-en-zh_hans0054435.003" class="semb x_xd1 hasSn">
<span class="gp x_xdh sn ty_label tg_semb">② </span><span class="trg x_xd2"><span class="ind">
<span class="gp tg_ind">(</span>computer<span class="gp tg_ind">) </span></span>
<span d:def="1" class="trans">笔记本电脑 <d:def></d:def></span>
<span class="trans ty_pinyin">bǐjìběn diànnǎo</span></span>
<span class="exg x_xd2 hasSn"><span class="x_xdh"><span class="sn">▸ </span><span class="ex">a notebook computer <span class="underline">or</span> PC </span></span>
<span class="trg x_xd3"><span class="trans">一台笔记本电脑 </span></span></span></span></span></d:entry></body></html>
    "#;

    #[test]
    fn test_html_parsing_and_styling() {
        let attr_string = html_to_attributed_string(DICTIONARY_HTML)
            .expect("Function should successfully return an NSMutableAttributedString");

        let ns_string: Retained<NSString> = attr_string.string();
        let rust_string = ns_string.to_string();
        assert!(
            rust_string.contains("notebook"),
            "The parsed string should contain the headword"
        );
        assert!(
            rust_string.contains("noun"),
            "The parsed string should contain the part of speech"
        );
        assert!(
            rust_string.contains("笔记本"),
            "The parsed string should contain the translation"
        );

        assert!(
            !rust_string.contains("span class="),
            "The parsed string should not contain raw HTML tags"
        );
    }
}
