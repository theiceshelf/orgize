use crate::elements::*;
use crate::iter::Iter;

use indextree::{Arena, NodeId};
use jetscii::bytes;
use memchr::{memchr_iter, memrchr_iter};

pub struct Org<'a> {
    pub(crate) arena: Arena<Element<'a>>,
    pub(crate) document: NodeId,
    root: Option<NodeId>,
    text: &'a str,
}

impl<'a> Org<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut arena = Arena::new();
        let document = arena.new_node(Element::Document {
            begin: 0,
            end: text.len(),
        });

        Org {
            arena,
            root: None,
            document,
            text,
        }
    }

    pub fn finish(&self) -> bool {
        self.arena[self.document].first_child().is_some()
    }

    pub fn iter(&'a mut self) -> Iter<'a> {
        if let Some(root) = self.root {
            Iter::new(&self.arena, root)
        } else {
            let root = self.arena.new_node(Element::Root);
            root.append(self.document, &mut self.arena).unwrap();
            Iter::new(&self.arena, root)
        }
    }

    pub fn parse(&mut self) {
        if self.finish() {
            return;
        }

        let mut node = self.document;
        loop {
            match self.arena[node].data {
                Element::Document { begin, end, .. }
                | Element::Headline {
                    contents_begin: begin,
                    contents_end: end,
                    ..
                } => {
                    let mut begin = begin;
                    if begin < end {
                        let off = Headline::find_level(&self.text[begin..end], std::usize::MAX);
                        if off != 0 {
                            let (contents_begin, contents_end) =
                                skip_empty_lines(&self.text[begin..begin + off]);
                            let section = Element::Section {
                                begin,
                                end: begin + off,
                                contents_begin: begin + contents_begin,
                                contents_end: begin + contents_end,
                            };
                            let new_node = self.arena.new_node(section);
                            node.append(new_node, &mut self.arena).unwrap();
                            begin += off;
                        }
                    }
                    while begin < end {
                        let (headline, off, end) = Headline::parse(&self.text[begin..end], &[]);
                        let headline = Element::Headline {
                            headline,
                            begin,
                            end: begin + end,
                            contents_begin: begin + off,
                            contents_end: begin + end,
                        };
                        let new_node = self.arena.new_node(headline);
                        node.append(new_node, &mut self.arena).unwrap();
                        begin += end;
                    }
                }
                Element::Section {
                    contents_begin,
                    contents_end,
                    ..
                }
                | Element::Block {
                    contents_begin,
                    contents_end,
                    ..
                }
                | Element::ListItem {
                    contents_begin,
                    contents_end,
                    ..
                } => {
                    self.parse_elements_children(contents_begin, contents_end, node);
                }
                Element::Paragraph {
                    contents_begin,
                    contents_end,
                    ..
                }
                | Element::Bold {
                    contents_begin,
                    contents_end,
                    ..
                }
                | Element::Underline {
                    contents_begin,
                    contents_end,
                    ..
                }
                | Element::Italic {
                    contents_begin,
                    contents_end,
                    ..
                }
                | Element::Strike {
                    contents_begin,
                    contents_end,
                    ..
                } => {
                    self.parse_objects_children(contents_begin, contents_end, node);
                }
                Element::List {
                    list: List { indent, .. },
                    contents_begin,
                    contents_end,
                    ..
                } => {
                    self.parse_list_items(contents_begin, contents_end, indent, node);
                }
                _ => (),
            }

            if let Some(next_node) = self.next_node(node) {
                node = next_node;
            } else {
                break;
            }
        }
    }

    fn next_node(&self, mut node: NodeId) -> Option<NodeId> {
        if let Some(child) = self.arena[node].first_child() {
            return Some(child);
        }

        loop {
            if let Some(sibling) = self.arena[node].next_sibling() {
                return Some(sibling);
            } else if let Some(parent) = self.arena[node].parent() {
                node = parent;
            } else {
                return None;
            }
        }
    }

    fn parse_elements_children(&mut self, mut begin: usize, end: usize, node: NodeId) {
        'out: while begin < end {
            let text = &self.text[begin..end];

            if let Some((ty, off)) = self.parse_element(begin, end) {
                let new_node = self.arena.new_node(ty);
                node.append(new_node, &mut self.arena).unwrap();
                begin += off;
                continue 'out;
            }

            let mut pos = 0;
            for i in memchr_iter(b'\n', text.as_bytes()) {
                if text.as_bytes()[pos..i].iter().all(u8::is_ascii_whitespace) {
                    let (end, _) = skip_empty_lines(&text[i..]);
                    let new_node = self.arena.new_node(Element::Paragraph {
                        begin,
                        end: begin + i + end,
                        contents_begin: begin,
                        contents_end: begin + pos,
                    });
                    node.append(new_node, &mut self.arena).unwrap();
                    begin += i + end;
                    continue 'out;
                } else if let Some((ty, off)) = self.parse_element(begin + pos, end) {
                    let new_node = self.arena.new_node(Element::Paragraph {
                        begin,
                        end: begin + pos,
                        contents_begin: begin,
                        contents_end: begin + pos,
                    });
                    node.append(new_node, &mut self.arena).unwrap();
                    let new_node = self.arena.new_node(ty);
                    node.append(new_node, &mut self.arena).unwrap();
                    begin += pos + off;
                    continue 'out;
                }
                pos = i + 1;
            }

            let new_node = self.arena.new_node(Element::Paragraph {
                begin,
                end,
                contents_begin: begin,
                contents_end: if text.ends_with('\n') { end - 1 } else { end },
            });
            begin = end;
            node.append(new_node, &mut self.arena).unwrap();
        }
    }

    fn parse_element(&self, begin: usize, end: usize) -> Option<(Element<'a>, usize)> {
        let text = &self.text[begin..end];

        if let Some((fn_def, off, end)) = FnDef::parse(text) {
            let fn_def = Element::FnDef {
                begin,
                end: begin + end,
                contents_begin: begin + off,
                contents_end: begin + end,
                fn_def,
            };
            return Some((fn_def, end));
        } else if let Some((list, limit, end)) = List::parse(text) {
            let list = Element::List {
                list,
                begin,
                end: begin + end,
                contents_begin: begin,
                contents_end: begin + limit,
            };
            return Some((list, end));
        }

        let line_begin = text.find(|c: char| !c.is_ascii_whitespace()).unwrap_or(0);
        let tail = &text[line_begin..];

        if let Some((clock, end)) = Clock::parse(tail) {
            let clock = Element::Clock {
                clock,
                begin,
                end: begin + line_begin + end,
            };
            return Some((clock, line_begin + end));
        }

        // TODO: LaTeX environment
        if tail.starts_with("\\begin{") {}

        // rule
        if tail.starts_with("-----") {
            if let Some(end) = Rule::parse(tail) {
                let rule = Element::Rule {
                    begin,
                    end: begin + line_begin + end,
                };
                return Some((rule, line_begin + end));
            }
        }

        if tail.starts_with(':') {
            if let Some((drawer, off, limit, end)) = Drawer::parse(tail) {
                let drawer = Element::Drawer {
                    drawer,
                    begin,
                    end: begin + line_begin + end,
                    contents_begin: begin + line_begin + off,
                    contents_end: begin + line_begin + limit,
                };
                return Some((drawer, line_begin + end));
            }
        }

        // fixed width
        if tail.starts_with(": ") || tail.starts_with(":\n") {
            // let end = line_ends
            //     .skip_while(|&i| {
            //         text[i + 1..].starts_with(": ") || text[i + 1..].starts_with(":\n")
            //     })
            //     .next()
            //     .map(|i| i + 1)
            //     .unwrap_or_else(|| text.len());
            // let off = end - pos;
            // brk!(Element::FixedWidth(&tail[0..off]), off);
        }

        // comment
        if tail.starts_with("# ") || tail.starts_with("#\n") {
            // let end = line_ends
            //     .skip_while(|&i| {
            //         text[i + 1..].starts_with("# ") || text[i + 1..].starts_with("#\n")
            //     })
            //     .next()
            //     .map(|i| i + 1)
            //     .unwrap_or_else(|| text.len());
            // let off = end - pos;
            // brk!(Element::Comment(&tail[0..off]), off);
        }

        if tail.starts_with("#+") {
            if let Some((block, off, limit, end)) = Block::parse(tail) {
                let block = Element::Block {
                    block,
                    begin,
                    end: begin + line_begin + end,
                    contents_begin: begin + line_begin + off,
                    contents_end: begin + line_begin + limit,
                };
                return Some((block, line_begin + end));
            } else if let Some((dyn_block, off, limit, end)) = DynBlock::parse(tail) {
                let dyn_block = Element::DynBlock {
                    dyn_block,
                    begin,
                    end: begin + line_begin + end,
                    contents_begin: begin + line_begin + off,
                    contents_end: begin + line_begin + limit,
                };
                return Some((dyn_block, line_begin + end));
            } else if let Some((key, option, value, end)) = Keyword::parse(tail) {
                if key.eq_ignore_ascii_case("CALL") {
                    let call = Element::BabelCall {
                        value,
                        begin,
                        end: begin + line_begin + end,
                    };
                    return Some((call, line_begin + end));
                } else {
                    let kw = Element::Keyword {
                        keyword: Keyword { key, option, value },
                        begin,
                        end: begin + line_begin + end,
                    };
                    return Some((kw, line_begin + end));
                }
            }
        }

        None
    }

    fn parse_objects_children(&mut self, mut begin: usize, end: usize, node: NodeId) {
        'out: while begin < end {
            let bytes = self.text[begin..end].as_bytes();

            match bytes[0] {
                b'{' | b' ' | b'"' | b',' | b'(' | b'\n' => {
                    if let Some((ty, off)) = self.parse_object(begin + 1, end) {
                        let new_node = self.arena.new_node(Element::Text {
                            value: &self.text[begin..=begin],
                            begin,
                            end,
                        });
                        node.append(new_node, &mut self.arena).unwrap();
                        let new_node = self.arena.new_node(ty);
                        node.append(new_node, &mut self.arena).unwrap();
                        begin += 1 + off;
                        continue;
                    }
                }
                _ => {
                    if let Some((ty, off)) = self.parse_object(begin, end) {
                        let new_node = self.arena.new_node(ty);
                        node.append(new_node, &mut self.arena).unwrap();
                        begin += off;
                        continue;
                    }
                }
            }

            let bs = bytes!(b'@', b' ', b'"', b'(', b'\n', b'{', b'<', b'[');
            let mut pos = 0;
            while let Some(off) = bs.find(&bytes[pos..]) {
                pos += off;
                assert!(begin + pos <= end);
                match bytes[pos] {
                    b'{' | b' ' | b'"' | b',' | b'(' | b'\n' => {
                        if let Some((ty, off)) = self.parse_object(begin + pos + 1, end) {
                            let new_node = self.arena.new_node(Element::Text {
                                value: &self.text[begin..=begin + pos],
                                begin,
                                end,
                            });
                            node.append(new_node, &mut self.arena).unwrap();
                            let new_node = self.arena.new_node(ty);
                            node.append(new_node, &mut self.arena).unwrap();
                            begin += pos + 1 + off;
                            continue 'out;
                        }
                    }
                    _ => {
                        if let Some((ty, off)) = self.parse_object(begin + pos, end) {
                            let new_node = self.arena.new_node(Element::Text {
                                value: &self.text[begin..begin + pos],
                                begin,
                                end,
                            });
                            node.append(new_node, &mut self.arena).unwrap();
                            let new_node = self.arena.new_node(ty);
                            node.append(new_node, &mut self.arena).unwrap();
                            begin += pos + off;
                            continue 'out;
                        }
                    }
                }
                pos += 1;
            }

            let new_node = self.arena.new_node(Element::Text {
                value: &self.text[begin..end],
                begin,
                end,
            });
            node.append(new_node, &mut self.arena).unwrap();
            begin = end;
        }
    }

    fn parse_object(&self, begin: usize, end: usize) -> Option<(Element<'a>, usize)> {
        let text = &self.text[begin..end];
        if text.len() < 3 {
            None
        } else {
            let bytes = text.as_bytes();
            match bytes[0] {
                b'@' if bytes[1] == b'@' => Snippet::parse(text).map(|(snippet, off)| {
                    (
                        Element::Snippet {
                            snippet,
                            begin,
                            end: begin + off,
                        },
                        off,
                    )
                }),
                b'{' if bytes[1] == b'{' && bytes[2] == b'{' => {
                    Macros::parse(text).map(|(macros, off)| {
                        (
                            Element::Macros {
                                macros,
                                begin,
                                end: begin + off,
                            },
                            off,
                        )
                    })
                }
                b'<' if bytes[1] == b'<' => {
                    if bytes[2] == b'<' {
                        RadioTarget::parse(text).map(|(radio_target, off)| {
                            (
                                Element::RadioTarget {
                                    radio_target,
                                    begin,
                                    end: begin + off,
                                },
                                off,
                            )
                        })
                    } else {
                        Target::parse(text).map(|(target, off)| {
                            (
                                Element::Target {
                                    target,
                                    begin,
                                    end: begin + off,
                                },
                                off,
                            )
                        })
                    }
                }
                b'<' => Timestamp::parse_active(text)
                    .or_else(|| (Timestamp::parse_diary(text)))
                    .map(|(timestamp, off)| {
                        (
                            Element::Timestamp {
                                timestamp,
                                begin,
                                end: begin + off,
                            },
                            off,
                        )
                    }),
                b'[' => {
                    if text[1..].starts_with("fn:") {
                        FnRef::parse(text).map(|(fn_ref, off)| {
                            (
                                Element::FnRef {
                                    fn_ref,
                                    begin,
                                    end: begin + off,
                                },
                                off,
                            )
                        })
                    } else if bytes[1] == b'[' {
                        Link::parse(text).map(|(link, off)| {
                            (
                                Element::Link {
                                    link,
                                    begin,
                                    end: begin + off,
                                },
                                off,
                            )
                        })
                    } else {
                        Cookie::parse(text)
                            .map(|(cookie, off)| {
                                (
                                    Element::Cookie {
                                        cookie,
                                        begin,
                                        end: begin + off,
                                    },
                                    off,
                                )
                            })
                            .or_else(|| {
                                Timestamp::parse_inactive(text).map(|(timestamp, off)| {
                                    (
                                        Element::Timestamp {
                                            timestamp,
                                            begin,
                                            end: begin + off,
                                        },
                                        off,
                                    )
                                })
                            })
                    }
                }
                b'*' => emphasis::parse(text, b'*').map(|off| {
                    (
                        Element::Bold {
                            begin,
                            contents_begin: begin + 1,
                            contents_end: begin + off - 1,
                            end: begin + off,
                        },
                        off,
                    )
                }),
                b'+' => emphasis::parse(text, b'+').map(|off| {
                    (
                        Element::Strike {
                            begin,
                            contents_begin: begin + 1,
                            contents_end: begin + off - 1,
                            end: begin + off,
                        },
                        off,
                    )
                }),
                b'/' => emphasis::parse(text, b'/').map(|off| {
                    (
                        Element::Italic {
                            begin,
                            contents_begin: begin + 1,
                            contents_end: begin + off - 1,
                            end: begin + off,
                        },
                        off,
                    )
                }),
                b'_' => emphasis::parse(text, b'_').map(|off| {
                    (
                        Element::Underline {
                            begin,
                            contents_begin: begin + 1,
                            contents_end: begin + off - 1,
                            end: begin + off,
                        },
                        off,
                    )
                }),
                b'=' => emphasis::parse(text, b'=').map(|off| {
                    (
                        Element::Verbatim {
                            begin,
                            end: begin + off,
                            value: &text[1..off - 1],
                        },
                        off,
                    )
                }),
                b'~' => emphasis::parse(text, b'~').map(|off| {
                    (
                        Element::Code {
                            begin,
                            end: begin + off,
                            value: &text[1..off - 1],
                        },
                        off,
                    )
                }),
                b's' if text.starts_with("src_") => {
                    InlineSrc::parse(text).map(|(inline_src, off)| {
                        (
                            Element::InlineSrc {
                                inline_src,
                                begin,
                                end: begin + off,
                            },
                            off,
                        )
                    })
                }
                b'c' if text.starts_with("call_") => {
                    InlineCall::parse(text).map(|(inline_call, off)| {
                        (
                            Element::InlineCall {
                                inline_call,
                                begin,
                                end: begin + off,
                            },
                            off,
                        )
                    })
                }
                _ => None,
            }
        }
    }

    fn parse_list_items(&mut self, mut begin: usize, end: usize, indent: usize, node: NodeId) {
        while begin < end {
            let text = &self.text[begin..end];
            let (list_item, off, end) = ListItem::parse(text, indent);
            let list_item = Element::ListItem {
                list_item,
                begin,
                end: begin + end,
                contents_begin: begin + off,
                contents_end: begin + end,
            };
            let new_node = self.arena.new_node(list_item);
            node.append(new_node, &mut self.arena).unwrap();
            begin += end;
        }
    }
}

fn skip_empty_lines(text: &str) -> (usize, usize) {
    let mut i = 0;
    let mut j = text.len();
    for pos in memchr_iter(b'\n', text.as_bytes()) {
        if text.as_bytes()[i..pos].iter().all(u8::is_ascii_whitespace) {
            i = pos + 1;
        } else {
            break;
        }
    }

    for pos in memrchr_iter(b'\n', text.as_bytes()) {
        if text.as_bytes()[pos..j].iter().all(u8::is_ascii_whitespace) {
            j = pos;
        } else {
            break;
        }
    }

    (i, j)
}