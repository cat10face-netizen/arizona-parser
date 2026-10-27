//! Поиск упоминаний ников администратора в тексте.

/// Оставляет только буквы/цифры в нижнем регистре.
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Ищет любой из ников в тексте (нечувствительно к регистру и `_ . - пробел`).
pub fn contains_admin_nick(text: &str, nicks: &[String]) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    // разбиваем текст на слова: последовательности алфанумерики
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(canon)
        .collect();
    if words.is_empty() {
        return None;
    }

    for nick in nicks {
        let parts: Vec<String> = nick
            .split(|c: char| !c.is_alphanumeric())
            .filter(|p| !p.is_empty())
            .map(canon)
            .collect();
        if parts.is_empty() {
            continue;
        }

        if parts.len() == 1 {
            // ник из одного слова (напр. "Torenno"): ищем точное слово
            let target = &parts[0];
            if target.len() < 4 {
                continue;
            }
            if words.iter().any(|w| w == target) {
                return Some(nick.clone());
            }
        } else {
            // ник из нескольких частей, напр. ["richard", "torenno"]
            // допускаем: подряд в отдельных словах ЛИБО одной строкой в тексте
            let joined_target: String = parts.concat();
            if canon(text).contains(&joined_target) {
                return Some(nick.clone());
            }
            if words.len() >= parts.len() {
                for i in 0..=(words.len() - parts.len()) {
                    if words[i..i + parts.len()] == parts[..] {
                        return Some(nick.clone());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nicks() -> Vec<String> {
        vec![
            "Richard_Torenno".to_string(),
            "R.Torenno".to_string(),
            "Richard Torenno".to_string(),
        ]
    }

    #[test]
    fn finds_underscore() {
        assert!(contains_admin_nick("жалоба на Richard_Torenno за бан", &nicks()).is_some());
    }

    #[test]
    fn finds_dot() {
        assert!(contains_admin_nick("админ R.Torenno выдал бан", &nicks()).is_some());
    }

    #[test]
    fn finds_space_and_case() {
        assert!(contains_admin_nick("игровой ник: richard torenno", &nicks()).is_some());
    }

    #[test]
    fn finds_in_form_field() {
        let text = "Тип жалобы: Жалоба на администрацию\nИгровой ник администратора: Richard_Torenno\nПричина: дб";
        assert!(contains_admin_nick(text, &nicks()).is_some());
    }

    #[test]
    fn no_false_positive() {
        assert!(contains_admin_nick("жалоба на другого админа John_Styles", &nicks()).is_none());
    }
}
