//! Borrowed macro templates: count copies before creating output tokens.
use crate::expansion_budget::{Cost, ExpansionBudget};
use crate::ParseResult;
use ratex_lexer::token::SourceLocation;
use ratex_lexer::{Lexer, Token};

#[derive(Clone, Copy)]
pub(super) enum Template<'a> {
    Text(&'a str),
    Tokens(&'a [Token]),
}
#[derive(Clone)]
struct View<'a> {
    text: &'a str,
    loc: SourceLocation,
    noexpand: bool,
    treat_as_relax: bool,
}
impl View<'_> {
    fn owned(self) -> Token {
        Token {
            text: self.text.to_owned(),
            loc: self.loc,
            noexpand: self.noexpand,
            treat_as_relax: self.treat_as_relax,
        }
    }
}
enum Iter<'a> {
    Text(Lexer<'a>),
    Tokens(std::iter::Rev<std::slice::Iter<'a, Token>>),
}
impl<'a> Iterator for Iter<'a> {
    type Item = View<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        Some(match self {
            Self::Text(lexer) => {
                let s = lexer.next_span();
                if s.is_eof {
                    return None;
                }
                View {
                    text: s.text,
                    loc: s.loc,
                    noexpand: false,
                    treat_as_relax: false,
                }
            }
            Self::Tokens(tokens) => {
                let t = tokens.next()?;
                View {
                    text: &t.text,
                    loc: t.loc.clone(),
                    noexpand: t.noexpand,
                    treat_as_relax: t.treat_as_relax,
                }
            }
        })
    }
}
enum Item<'a> {
    Literal(View<'a>),
    Arg(usize),
}
struct Items<'a> {
    iter: std::iter::Peekable<Iter<'a>>,
    num_args: usize,
}
impl<'a> Iterator for Items<'a> {
    type Item = Item<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let t = self.iter.next()?;
        if self.num_args > 0 && t.text == "#" {
            if let Some(next) = self.iter.peek() {
                if next.text == "#" {
                    // Preserve the second hash's location: mhchem uses source
                    // gaps to reconstruct whitespace after macro substitution.
                    return self.iter.next().map(Item::Literal);
                } else if let Ok(n) = next.text.parse::<usize>() {
                    if n > 0 && n <= self.num_args {
                        self.iter.next();
                        return Some(Item::Arg(n - 1));
                    }
                }
            }
        }
        Some(Item::Literal(t))
    }
}
impl<'a> Template<'a> {
    fn iter(self) -> Iter<'a> {
        match self {
            Self::Text(s) => Iter::Text(Lexer::new(s)),
            Self::Tokens(t) => Iter::Tokens(t.iter().rev()),
        }
    }
    fn items(self, num_args: usize) -> Items<'a> {
        Items {
            iter: self.iter().peekable(),
            num_args,
        }
    }

    // Preserve the Text API's historical raw-text inference (including hashes
    // in comments/control words), without allocating text.replace("##", "").
    pub(super) fn num_args(self) -> usize {
        let Self::Text(text) = self else {
            unreachable!("token templates carry an explicit arity")
        };
        if !text.contains('#') {
            return 0;
        }
        let mut num_args = 0usize;
        loop {
            let Some(next) = num_args.checked_add(1) else {
                return num_args;
            };
            let needle = format!("#{next}"); // bounded arity metadata, not body text
            let mut matched = 0;
            let found = text.split("##").flat_map(str::bytes).any(|byte| {
                if byte == needle.as_bytes()[matched] {
                    matched += 1;
                } else {
                    matched = usize::from(byte == b'#');
                }
                matched == needle.len()
            });
            if !found {
                return num_args;
            }
            num_args = next;
        }
    }

    pub(super) fn expand(
        self,
        mut args: Vec<Vec<Token>>,
        budget: &ExpansionBudget,
    ) -> ParseResult<Vec<Token>> {
        let mut remaining = vec![0usize; args.len()];
        let mut cost = Some(Cost::default());
        for item in self.items(args.len()) {
            match item {
                Item::Literal(t) => cost = cost.and_then(|c| c.checked_add(Cost::token(t.text))),
                Item::Arg(n) => {
                    remaining[n] = match remaining[n].checked_add(1) {
                        Some(n) => n,
                        None => {
                            budget.charge(None, "parameter references")?;
                            unreachable!()
                        }
                    };
                }
            }
        }
        for (n, arg) in args.iter().enumerate() {
            let copies = remaining[n].saturating_sub(1);
            let arg_cost = token_cost(arg).and_then(|c| c.checked_mul(copies));
            cost = cost.and_then(|c| arg_cost.and_then(|a| c.checked_add(a)));
        }
        budget.charge(cost, "macro replacement")?;
        let mut out = Vec::new();
        for item in self.items(args.len()) {
            match item {
                Item::Literal(t) => out.push(t.owned()),
                Item::Arg(n) => {
                    remaining[n] -= 1;
                    if remaining[n] == 0 {
                        out.extend(std::mem::take(&mut args[n]).into_iter().rev());
                    } else {
                        out.extend(args[n].iter().rev().cloned());
                    }
                }
            }
        }
        out.reverse();
        Ok(out)
    }
}
pub(super) fn token_cost(tokens: &[Token]) -> Option<Cost> {
    tokens
        .iter()
        .try_fold(Cost::default(), |c, t| c.checked_add(Cost::token(&t.text)))
}
