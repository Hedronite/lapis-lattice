//! Tiny authored PDFs. Not book text.

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{Bookmark, Document, Object, Stream, dictionary};

pub struct Mark {
    pub title: &'static str,
    pub page: usize,
    pub parent: Option<usize>,
}

pub fn write(path: &Path, pages: &[Vec<&str>], marks: &[Mark]) {
    let sized: Vec<Vec<(&str, f32)>> =
        pages.iter().map(|lines| lines.iter().copied().map(|line| (line, 12.0)).collect()).collect();
    write_sized(path, &sized, marks);
}

/// Same as [`write`], with a font size per line. No outline when `marks` is empty.
pub fn write_sized(path: &Path, pages: &[Vec<(&str, f32)>], marks: &[Mark]) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let mut page_ids = Vec::new();
    for lines in pages {
        let content_id = doc.add_object(page_stream_sized(lines));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        page_ids.push(page_id);
    }
    let kids: Vec<Object> = page_ids.iter().copied().map(Object::from).collect();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_ids.len() as i64,
        }),
    );
    let mut ids = Vec::new();
    for mark in marks {
        let parent = mark.parent.map(|i| ids[i]);
        let id = doc
            .add_bookmark(Bookmark::new(mark.title.into(), [0.0, 0.0, 0.0], 0, page_ids[mark.page]), parent);
        ids.push(id);
    }
    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if !marks.is_empty()
        && let Some(outline_id) = doc.build_outline()
    {
        catalog.set("Outlines", outline_id);
    }
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

fn page_stream_sized(lines: &[(&str, f32)]) -> Stream {
    let mut ops = vec![Operation::new("BT", vec![])];
    for (i, (line, size)) in lines.iter().enumerate() {
        ops.push(Operation::new("Tf", vec!["F1".into(), (*size as i64).into()]));
        if i == 0 {
            ops.push(Operation::new("Td", vec![72.into(), 720.into()]));
        } else {
            ops.push(Operation::new("Td", vec![0.into(), (-18).into()]));
        }
        ops.push(Operation::new("Tj", vec![Object::string_literal(line.as_bytes().to_vec())]));
    }
    ops.push(Operation::new("ET", vec![]));
    Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap())
}

fn page_stream(lines: &[&str]) -> Stream {
    let mut ops = vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec!["F1".into(), 12.into()]),
        Operation::new("Td", vec![72.into(), 720.into()]),
    ];
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            ops.push(Operation::new("Td", vec![0.into(), (-16).into()]));
        }
        ops.push(Operation::new("Tj", vec![Object::string_literal(line.as_bytes().to_vec())]));
    }
    ops.push(Operation::new("ET", vec![]));
    Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap())
}

/// Drop the GoTo action on the outline item whose title matches, so the item has no destination.
pub fn strip_outline_action(path: &Path, title: &str) {
    let mut doc = Document::load(path).unwrap();
    let ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(Object::Dictionary(dict)) = doc.objects.get(&id) else {
            continue;
        };
        let Ok(title_obj) = dict.get(b"Title") else {
            continue;
        };
        let Ok(text) = lopdf::decode_text_string(title_obj) else {
            continue;
        };
        if text == title {
            let mut dict = dict.clone();
            dict.remove(b"A");
            dict.remove(b"Dest");
            doc.objects.insert(id, Object::Dictionary(dict));
        }
    }
    doc.save(path).unwrap();
}

/// Page tree whose `/Kids` includes the pages node itself.
pub fn write_cyclic_page_tree(path: &Path) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(page_stream(&["sparse"]));
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(pages_id), Object::Reference(page_id)],
            "Count" => 1,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// One page whose only content is a Form XObject that `Do`s itself.
pub fn write_cyclic_form(path: &Path) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let form_id = doc.new_object_id();
    let form_bytes = Content { operations: vec![Operation::new("Do", vec!["Fm".into()])] }.encode().unwrap();
    let form = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Fm" => form_id },
            },
        },
        form_bytes,
    );
    doc.objects.insert(form_id, Object::Stream(form));
    let content_id = doc.add_object(Stream::new(
        dictionary! {},
        Content { operations: vec![Operation::new("Do", vec!["Fm".into()])] }.encode().unwrap(),
    ));
    let resources_id = doc.add_object(dictionary! {
        "XObject" => dictionary! { "Fm" => form_id },
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// Page 1 has a `Tj` operand pdf-extract panics on. Page 2 is ordinary text.
pub fn write_panic_page(path: &Path) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let bad = {
        let ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Tj", vec![Object::Integer(1)]),
            Operation::new("ET", vec![]),
        ];
        doc.add_object(Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap()))
    };
    let good = doc.add_object(page_stream(&["gamma marker lives on the good page."]));
    let mut page_ids = Vec::new();
    for content_id in [bad, good] {
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        page_ids.push(page_id);
    }
    let kids: Vec<Object> = page_ids.iter().copied().map(Object::from).collect();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_ids.len() as i64,
        }),
    );
    let outline = doc.add_bookmark(Bookmark::new("Chapter".into(), [0.0, 0.0, 0.0], 0, page_ids[1]), None);
    let _ = outline;
    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if let Some(outline_id) = doc.build_outline() {
        catalog.set("Outlines", outline_id);
    }
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// One outline item whose `/Next` points at itself.
pub fn write_cyclic_outline(path: &Path) {
    write(
        path,
        &[vec!["CHAPTER 1", "A real heading would not matter."]],
        &[Mark { title: "Chapter", page: 0, parent: None }],
    );
    let mut doc = Document::load(path).unwrap();
    let ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(Object::Dictionary(dict)) = doc.objects.get(&id) else {
            continue;
        };
        if dict.get(b"Title").is_err() {
            continue;
        }
        let mut dict = dict.clone();
        dict.set("Next", Object::Reference(id));
        doc.objects.insert(id, Object::Dictionary(dict));
        break;
    }
    doc.save(path).unwrap();
}

/// `/Kids` stored as an indirect array. A direct `get` misses it.
pub fn write_indirect_kids(path: &Path) {
    write(path, &[vec!["indirect marker"]], &[Mark { title: "Chapter", page: 0, parent: None }]);
    let mut doc = Document::load(path).unwrap();
    let pages_id = doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
    let kids = doc.get_dictionary(pages_id).unwrap().get(b"Kids").unwrap().clone();
    let kids_id = doc.add_object(kids);
    let mut dict = doc.get_dictionary(pages_id).unwrap().clone();
    dict.set("Kids", Object::Reference(kids_id));
    doc.objects.insert(pages_id, Object::Dictionary(dict));
    doc.save(path).unwrap();
}

/// Eighty nested `/Pages` nodes. Deeper than 64, still a real page.
pub fn write_deep_page_tree(path: &Path) {
    write(path, &[vec!["deep marker"]], &[Mark { title: "Chapter", page: 0, parent: None }]);
    let mut doc = Document::load(path).unwrap();
    let mut current = doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
    for _ in 0..80 {
        let id = doc.new_object_id();
        let dict = doc.get_dictionary(current).unwrap().clone();
        doc.objects.insert(id, Object::Dictionary(dict));
        let mut parent = doc.get_dictionary(current).unwrap().clone();
        parent.set("Kids", vec![Object::Reference(id)]);
        parent.set("Count", 1);
        parent.set("Type", "Pages");
        doc.objects.insert(current, Object::Dictionary(parent));
        current = id;
    }
    doc.save(path).unwrap();
}

/// Page text plus a `Do` of an Image XObject that points at itself.
/// pdf-extract recurses into every `Do`, not only Forms.
pub fn write_image_do(path: &Path) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let image_id = doc.new_object_id();
    let image_ops = Content { operations: vec![Operation::new("Do", vec!["Im".into()])] }.encode().unwrap();
    doc.objects.insert(
        image_id,
        Object::Stream(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
                "Resources" => dictionary! {
                    "XObject" => dictionary! { "Im" => image_id },
                },
            },
            image_ops,
        )),
    );
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let content_id = doc.add_object(Stream::new(
        dictionary! {},
        Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Tj", vec![Object::string_literal(b"gamma marker".to_vec())]),
                Operation::new("ET", vec![]),
                Operation::new("Do", vec!["Im".into()]),
            ],
        }
        .encode()
        .unwrap(),
    ));
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Im" => image_id },
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }),
    );
    let outline_id = doc.add_bookmark(Bookmark::new("Chapter".into(), [0.0, 0.0, 0.0], 0, page_id), None);
    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if let Some(id) = doc.build_outline() {
        catalog.set("Outlines", id);
    }
    let _ = outline_id;
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// One bookmark whose destination name is not in the name tree.
pub fn write_broken_dest(path: &Path) {
    write(
        path,
        &[vec!["alpha body"], vec!["beta body"]],
        &[Mark { title: "Alpha", page: 0, parent: None }, Mark { title: "Beta", page: 1, parent: None }],
    );
    let mut doc = Document::load(path).unwrap();
    let ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(Object::Dictionary(dict)) = doc.objects.get(&id) else {
            continue;
        };
        let Ok(title) = dict.get(b"Title") else {
            continue;
        };
        let Ok(text) = lopdf::decode_text_string(title) else {
            continue;
        };
        if text == "Alpha" {
            let mut dict = dict.clone();
            dict.set("Dest", Object::Name(b"NoSuchDest".to_vec()));
            dict.remove(b"A");
            doc.objects.insert(id, Object::Dictionary(dict));
            break;
        }
    }
    doc.save(path).unwrap();
}

pub fn prose(n: usize) -> Vec<Vec<&'static str>> {
    let mut pages = Vec::with_capacity(n);
    for _ in 0..n {
        pages.push(vec!["the quick brown fox jumps over the lazy dog and keeps talking about nothing."]);
    }
    pages
}
