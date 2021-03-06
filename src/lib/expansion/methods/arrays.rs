use super::{
    super::{
        is_expression,
        words::{Select, SelectWithSize},
        Error, Expander, Index,
    },
    strings::unescape,
    MethodError, Pattern,
};
use crate::types::{self, Args};
use std::char;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, PartialEq, Clone)]
pub struct ArrayMethod<'a> {
    method:    &'a str,
    variable:  &'a str,
    pattern:   Pattern<'a>,
    selection: Select<types::Str>,
}

impl<'a> ArrayMethod<'a> {
    pub const fn new(
        method: &'a str,
        variable: &'a str,
        pattern: Pattern<'a>,
        selection: Select<types::Str>,
    ) -> Self {
        Self { method, variable, pattern, selection }
    }

    fn reverse<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        let mut result = self.resolve_array(expand_func)?;
        result.reverse();
        Ok(result)
    }

    fn lines<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        Ok(self.resolve_var(expand_func)?.lines().map(types::Str::from).collect())
    }

    fn chars<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        let variable = self.resolve_var(expand_func)?;
        let len = variable.chars().count();
        Ok(variable.chars().map(|c| types::Str::from(c.to_string())).select(&self.selection, len))
    }

    fn bytes<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        let variable = self.resolve_var(expand_func)?;
        let len = variable.len();
        Ok(variable.bytes().map(|b| types::Str::from(b.to_string())).select(&self.selection, len))
    }

    fn map_keys<'b, E: Expander>(&self, expand_func: &'b E) -> Result<Args, Error<E::Error>> {
        expand_func.map_keys(self.variable, &self.selection)
    }

    fn map_values<'b, E: Expander>(&self, expand_func: &'b E) -> Result<Args, Error<E::Error>> {
        expand_func.map_values(self.variable, &self.selection)
    }

    fn graphemes<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        let variable = self.resolve_var(expand_func)?;
        let graphemes: Vec<types::Str> =
            UnicodeSegmentation::graphemes(variable.as_str(), true).map(From::from).collect();
        let len = graphemes.len();
        Ok(graphemes.into_iter().select(&self.selection, len))
    }

    fn split_at<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        let variable = self.resolve_var(expand_func)?;
        match self.pattern {
            Pattern::StringPattern(string) => {
                if let Ok(value) = expand_func.expand_string(string)?.join(" ").parse::<usize>() {
                    if value < variable.len() {
                        let (l, r) = variable.split_at(value);
                        Ok(args![types::Str::from(l), types::Str::from(r)])
                    } else {
                        Err(Error::InvalidIndex(
                            Select::Index(Index::Forward(value)),
                            "array",
                            variable.to_string(),
                        ))
                    }
                } else {
                    Err(MethodError::WrongArgument(
                        "split_at",
                        "requires a valid number as an argument",
                    )
                    .into())
                }
            }
            Pattern::Whitespace => {
                Err(MethodError::WrongArgument("split_at", "requires an argument").into())
            }
        }
    }

    fn split<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        let variable = self.resolve_var(expand_func)?;
        let data: Args = match self.pattern {
            Pattern::Whitespace => variable
                .split(char::is_whitespace)
                .filter(|x| !x.is_empty())
                .map(From::from)
                .collect(),
            Pattern::StringPattern(pattern) => variable
                .split(unescape(&expand_func.expand_string(pattern)?.join(" ")).as_str())
                .map(From::from)
                .collect(),
        };
        let len = data.len();
        Ok(data.into_iter().select(&self.selection, len))
    }

    #[inline]
    fn resolve_array<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        match expand_func.array(self.variable, &Select::All) {
            Ok(array) => Ok(array),
            Err(Error::VarNotFound) if is_expression(self.variable) => {
                expand_func.expand_string(self.variable)
            }
            Err(why) => Err(why),
        }
    }

    #[inline]
    fn resolve_var<E: Expander>(&self, expand_func: &E) -> Result<types::Str, Error<E::Error>> {
        match expand_func.string(self.variable) {
            Ok(variable) => Ok(variable),
            Err(Error::VarNotFound) if is_expression(self.variable) => {
                Ok(types::Str::from_string(expand_func.expand_string(self.variable)?.join(" ")))
            }
            Err(why) => Err(why),
        }
    }

    pub fn handle_as_array<E: Expander>(&self, expand_func: &E) -> Result<Args, Error<E::Error>> {
        match self.method {
            "bytes" => self.bytes(expand_func),
            "chars" => self.chars(expand_func),
            "graphemes" => self.graphemes(expand_func),
            "keys" => self.map_keys(expand_func).map_err(Error::from),
            "lines" => self.lines(expand_func),
            "reverse" => self.reverse(expand_func),
            "split_at" => self.split_at(expand_func),
            "split" => self.split(expand_func),
            "values" => self.map_values(expand_func).map_err(Error::from),
            _ => Err(MethodError::InvalidArrayMethod(self.method.to_string()).into()),
        }
    }

    pub fn handle<E: Expander>(
        &self,
        current: &mut types::Str,
        expand_func: &E,
    ) -> Result<(), Error<E::Error>> {
        match self.method {
            "split" => {
                current.push_str(&self.split(expand_func)?.join(" "));
                Ok(())
            }
            _ => Err(MethodError::InvalidArrayMethod(self.method.to_string()).into()),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        expansion::test::DummyExpander,
        ranges::{Index, Range},
        types,
    };

    #[test]
    fn test_split_string_all() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new("split", "$FOO", Pattern::StringPattern("OB"), Select::All);
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "FO AR");
    }

    #[test]
    fn test_split_whitespace_all() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new("split", "$SPACEDFOO", Pattern::Whitespace, Select::All);
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "FOO BAR");
    }

    #[test]
    fn test_split_string_index_forward() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new(
            "split",
            "$FOO",
            Pattern::StringPattern("OB"),
            Select::Index(Index::Forward(1)),
        );
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "AR");
    }

    #[test]
    fn test_split_whitespace_index_forward() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new(
            "split",
            "$SPACEDFOO",
            Pattern::Whitespace,
            Select::Index(Index::Forward(1)),
        );
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "BAR");
    }

    #[test]
    fn test_split_string_index_backward() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new(
            "split",
            "$FOO",
            Pattern::StringPattern("OB"),
            Select::Index(Index::Backward(1)),
        );
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "FO");
    }

    #[test]
    fn test_split_whitespace_index_backward() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new(
            "split",
            "$SPACEDFOO",
            Pattern::Whitespace,
            Select::Index(Index::Backward(1)),
        );
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "FOO");
    }

    #[test]
    fn test_split_string_range() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new(
            "split",
            "$FOO",
            Pattern::StringPattern("OB"),
            Select::Range(Range::from(Index::Forward(0))),
        );
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "FO AR");
    }

    #[test]
    fn test_split_whitespace_range() {
        let mut output = types::Str::new();
        let method = ArrayMethod::new(
            "split",
            "$SPACEDFOO",
            Pattern::Whitespace,
            Select::Range(Range::from(Index::Forward(0))),
        );
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "FOO BAR");
    }

    #[test]
    fn test_split_key() {
        let mut output = types::Str::new();
        let method =
            ArrayMethod::new("split", "$SPACEDFOO", Pattern::Whitespace, Select::Key("1".into()));
        method.handle(&mut output, &DummyExpander).unwrap();
        assert_eq!(&*output, "");
    }

    #[test]
    fn test_split_at_failing_whitespace() {
        let method = ArrayMethod::new("split_at", "$SPACEDFOO", Pattern::Whitespace, Select::All);
        assert!(method.handle_as_array(&DummyExpander).is_err());
    }

    #[test]
    fn test_split_at_failing_no_number() {
        let method =
            ArrayMethod::new("split_at", "$SPACEDFOO", Pattern::StringPattern("a"), Select::All);
        assert!(method.handle_as_array(&DummyExpander).is_err());
    }

    #[test]
    fn test_split_at_failing_out_of_bound() {
        let method =
            ArrayMethod::new("split_at", "$SPACEDFOO", Pattern::StringPattern("100"), Select::All);
        assert!(method.handle_as_array(&DummyExpander).is_err());
    }

    #[test]
    fn test_split_at_succeeding() {
        let method = ArrayMethod::new("split_at", "$FOO", Pattern::StringPattern("3"), Select::All);
        assert_eq!(method.handle_as_array(&DummyExpander).unwrap(), args!["FOO", "BAR"]);
    }

    #[test]
    fn test_graphemes() {
        let method =
            ArrayMethod::new("graphemes", "$FOO", Pattern::StringPattern("3"), Select::All);
        assert_eq!(
            method.handle_as_array(&DummyExpander).unwrap(),
            args!["F", "O", "O", "B", "A", "R"]
        );
    }

    #[test]
    fn test_bytes() {
        let method = ArrayMethod::new("bytes", "$FOO", Pattern::StringPattern("3"), Select::All);
        assert_eq!(
            method.handle_as_array(&DummyExpander).unwrap(),
            args!["70", "79", "79", "66", "65", "82"]
        );
    }

    #[test]
    fn test_chars() {
        let method = ArrayMethod::new("chars", "$FOO", Pattern::StringPattern("3"), Select::All);
        assert_eq!(
            method.handle_as_array(&DummyExpander).unwrap(),
            args!["F", "O", "O", "B", "A", "R"]
        );
    }

    #[test]
    fn test_lines() {
        let method =
            ArrayMethod::new("lines", "$MULTILINE", Pattern::StringPattern("3"), Select::All);
        assert_eq!(method.handle_as_array(&DummyExpander).unwrap(), args!["FOO", "BAR"]);
    }

    #[test]
    fn test_reverse() {
        let method =
            ArrayMethod::new("reverse", "@ARRAY", Pattern::StringPattern("3"), Select::All);
        assert_eq!(method.handle_as_array(&DummyExpander).unwrap(), args!["c", "b", "a"]);
    }
}
