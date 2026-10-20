//! The workspace's people, for `@` mentions.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Person {
    pub id: String,
    /// What Slack shows: the display name, else the full name.
    pub name: String,
    pub real_name: String,
    /// The username, without the `@`.
    pub handle: String,
    pub avatar_url: Option<String>,
}

impl Person {
    /// How a mention of this person reads in the composer.
    pub fn label(&self) -> String {
        format!("@{}", self.name)
    }

    pub fn markup(&self) -> String {
        format!("<@{}>", self.id)
    }
}

/// People matching what was typed after `@`, best first: those in
/// `preferred` (people the user talks to), then whose name starts with the
/// query, then those with a later word that does.
pub fn search<'a>(
    people: &'a [Person],
    query: &str,
    preferred: &[String],
    max: usize,
) -> Vec<&'a Person> {
    let query = query.to_lowercase();
    let mut found: Vec<(u8, &Person)> = people
        .iter()
        .filter_map(|person| {
            let rank = match_rank(person, &query)?;
            let known = preferred.contains(&person.id);
            if query.is_empty() && !known {
                return None;
            }
            Some((if known { rank } else { rank + 2 }, person))
        })
        .collect();
    found.sort_by(|(a_rank, a), (b_rank, b)| {
        a_rank
            .cmp(b_rank)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    found
        .into_iter()
        .take(max)
        .map(|(_, person)| person)
        .collect()
}

/// 0 when a name starts with the query, 1 when a later word does.
fn match_rank(person: &Person, query: &str) -> Option<u8> {
    let names = [&person.name, &person.real_name, &person.handle];
    let lower: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    if lower.iter().any(|name| name.starts_with(query)) {
        return Some(0);
    }
    lower
        .iter()
        .flat_map(|name| name.split([' ', '.', '-', '_']).skip(1))
        .any(|word| word.starts_with(query))
        .then_some(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, name: &str, real_name: &str, handle: &str) -> Person {
        Person {
            id: id.into(),
            name: name.into(),
            real_name: real_name.into(),
            handle: handle.into(),
            avatar_url: None,
        }
    }

    #[test]
    fn people_you_talk_to_and_name_starts_come_first() {
        let people = [
            person("U1", "Axel Wennström", "Axel Wennström", "axel"),
            person("U2", "Max", "Maximilian Axelsson", "max.a"),
            person("U3", "Alexandra", "Alexandra Berg", "alex"),
            person("U4", "Bo", "Bo Ek", "bo"),
        ];
        let ids = |found: Vec<&Person>| -> Vec<String> {
            found.iter().map(|person| person.id.clone()).collect()
        };
        assert_eq!(ids(search(&people, "ax", &[], 8)), ["U1", "U2"]);
        assert_eq!(ids(search(&people, "AX", &["U2".into()], 8)), ["U2", "U1"]);
        assert_eq!(ids(search(&people, "wenn", &[], 8)), ["U1"]);
        assert_eq!(ids(search(&people, "al", &[], 8)), ["U3"]);
        assert_eq!(
            ids(search(&people, "", &["U4".into(), "U1".into()], 8)),
            ["U1", "U4"],
            "right after @, the people you talk to"
        );
        assert_eq!(people[0].label(), "@Axel Wennström");
        assert_eq!(people[0].markup(), "<@U1>");
    }
}
