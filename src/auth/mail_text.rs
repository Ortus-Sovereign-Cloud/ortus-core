//! The text of the messages the core sends, in the six languages of
//! Ortus. The language is the one the person chose on the page.

/// The language to write in: one of en, pl, uk, de, fr, ar.
pub fn language(code: Option<&str>) -> &'static str {
    match code
        .unwrap_or("en")
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
        .as_str()
    {
        "pl" => "pl",
        "uk" => "uk",
        "de" => "de",
        "fr" => "fr",
        "ar" => "ar",
        _ => "en",
    }
}

/// The message with the code that finishes signing up.
pub fn signup_code(language: &str, code: &str, site: &str) -> (String, String) {
    let (subject, body) = match language {
        "pl" => ("Twój kod Ortus: {code}", "Twój kod, aby dokończyć zakładanie konta w Ortus ({site}), to:\n\n    {code}\n\nKod działa przez 15 minut. Jeśli to nie Ty zakładasz konto, zignoruj tę wiadomość."),
        "uk" => ("Ваш код Ortus: {code}", "Ваш код, щоб завершити реєстрацію в Ortus ({site}):\n\n    {code}\n\nКод діє 15 хвилин. Якщо ви не реєструвалися, просто проігноруйте цей лист."),
        "de" => ("Ihr Ortus-Code: {code}", "Ihr Code, um die Registrierung bei Ortus ({site}) abzuschließen:\n\n    {code}\n\nDer Code gilt 15 Minuten. Wenn Sie sich nicht registriert haben, ignorieren Sie diese Nachricht."),
        "fr" => ("Votre code Ortus : {code}", "Votre code pour terminer votre inscription à Ortus ({site}) :\n\n    {code}\n\nLe code est valable 15 minutes. Si vous ne vous êtes pas inscrit, ignorez ce message."),
        "ar" => ("رمز Ortus الخاص بك: {code}", "رمزك لإكمال إنشاء حسابك في Ortus ‏({site}):\n\n    {code}\n\nيعمل الرمز لمدة 15 دقيقة. إن لم تكن أنت من أنشأ الحساب، فتجاهل هذه الرسالة."),
        _ => ("Your Ortus code: {code}", "Your code to finish signing up for Ortus ({site}) is:\n\n    {code}\n\nIt works for 15 minutes. If you did not sign up, ignore this message."),
    };
    (
        subject.replace("{code}", code),
        body.replace("{code}", code).replace("{site}", site),
    )
}

/// The message to an address that already has an account, sent instead
/// of a code (so the sign-up form does not tell who has an account).
pub fn already_signed_up(language: &str, site: &str) -> (String, String) {
    let (subject, body) = match language {
        "pl" => ("Masz już konto Ortus", "Ktoś próbował założyć konto Ortus ({site}) na ten adres, ale konto już istnieje. Zaloguj się na {site}/sign-in. Jeśli to nie Ty, nic nie rób."),
        "uk" => ("У вас уже є обліковий запис Ortus", "Хтось спробував зареєструвати обліковий запис Ortus ({site}) на цю адресу, але він уже існує. Увійдіть на {site}/sign-in. Якщо це були не ви, нічого не робіть."),
        "de" => ("Sie haben bereits ein Ortus-Konto", "Jemand wollte mit dieser Adresse ein Ortus-Konto ({site}) anlegen, aber es gibt bereits eines. Melden Sie sich unter {site}/sign-in an. Wenn Sie das nicht waren, müssen Sie nichts tun."),
        "fr" => ("Vous avez déjà un compte Ortus", "Quelqu’un a voulu créer un compte Ortus ({site}) avec cette adresse, mais il existe déjà. Connectez-vous sur {site}/sign-in. Si ce n’était pas vous, vous n’avez rien à faire."),
        "ar" => ("لديك حساب Ortus بالفعل", "حاول أحدهم إنشاء حساب Ortus ‏({site}) بهذا العنوان، لكن الحساب موجود بالفعل. سجّل الدخول على {site}/sign-in. إن لم تكن أنت، فلا داعي لفعل أي شيء."),
        _ => ("You already have an Ortus account", "Someone tried to sign up for Ortus ({site}) with this address, but it already has an account. Sign in at {site}/sign-in. If this was not you, there is nothing to do."),
    };
    (subject.to_string(), body.replace("{site}", site))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_its_text() {
        for lang in ["en", "pl", "uk", "de", "fr", "ar"] {
            let (subject, body) = signup_code(lang, "123456", "https://ortus.cc");
            assert!(
                subject.contains("123456")
                    && body.contains("123456")
                    && body.contains("https://ortus.cc"),
                "{lang}"
            );
            let (_, body) = already_signed_up(lang, "https://ortus.cc");
            assert!(body.contains("https://ortus.cc/sign-in"), "{lang}");
        }
        assert_eq!("uk", language(Some("uk-UA")));
        assert_eq!("en", language(Some("ja")));
        assert_eq!("en", language(None));
    }
}
