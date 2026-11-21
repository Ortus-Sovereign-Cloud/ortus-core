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

/// The invitation to a new account. `account` names the account's address
/// when the letter goes to another one (a personal address).
pub fn invitation(
    language: &str,
    inviter: &str,
    link: &str,
    account: Option<&str>,
) -> (String, String) {
    let (subject, body) = match language {
        "pl" => ("{inviter} zaprasza Cię do Ortus", "{inviter} założył(a) dla Ciebie konto w Ortus.\n\nOtwórz ten link, aby je aktywować i dodać klucz dostępu:\n\n    {link}\n\nLink działa przez 7 dni i tylko raz."),
        "uk" => ("{inviter} запрошує вас до Ortus", "{inviter} створив(ла) для вас обліковий запис в Ortus.\n\nВідкрийте це посилання, щоб активувати його й додати ключ доступу:\n\n    {link}\n\nПосилання діє 7 днів і лише один раз."),
        "de" => ("{inviter} lädt Sie zu Ortus ein", "{inviter} hat ein Ortus-Konto für Sie angelegt.\n\nÖffnen Sie diesen Link, um es zu aktivieren und einen Passkey hinzuzufügen:\n\n    {link}\n\nDer Link gilt 7 Tage und nur einmal."),
        "fr" => ("{inviter} vous invite sur Ortus", "{inviter} vous a créé un compte Ortus.\n\nOuvrez ce lien pour l’activer et ajouter une clé d’accès :\n\n    {link}\n\nLe lien est valable 7 jours, une seule fois."),
        "ar" => ("يدعوك {inviter} إلى Ortus", "أنشأ {inviter} حسابًا لك في Ortus.\n\nافتح هذا الرابط لتفعيله وإضافة مفتاح مرور:\n\n    {link}\n\nيعمل الرابط لمدة 7 أيام ولمرة واحدة."),
        _ => ("{inviter} invites you to Ortus", "{inviter} made an Ortus account for you.\n\nOpen this link to activate it and add a passkey:\n\n    {link}\n\nThe link works for 7 days, once."),
    };
    let account_line = account.map(|account| {
        let line = match language {
            "pl" => "Twoje konto: {account}",
            "uk" => "Ваш обліковий запис: {account}",
            "de" => "Ihr Konto: {account}",
            "fr" => "Votre compte : {account}",
            "ar" => "حسابك: {account}",
            _ => "Your account: {account}",
        };
        format!("\n\n{}", line.replace("{account}", account))
    });
    let body = body.replace("{inviter}", inviter).replace("{link}", link);
    // After the first paragraph (who made the account).
    let body = match (account_line, body.split_once("\n\n")) {
        (Some(line), Some((first, rest))) => format!("{first}{line}\n\n{rest}"),
        _ => body,
    };
    (subject.replace("{inviter}", inviter), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_letter_to_another_address_names_the_account() {
        for lang in ["en", "pl", "uk", "de", "fr", "ar"] {
            let (_, body) = invitation(
                lang,
                "Taras",
                "https://ortus.cc/welcome/accept#t",
                Some("iryna@ministry.example"),
            );
            let (first, rest) = body.split_once("\n\n").unwrap();
            assert!(
                first.contains("Taras")
                    && rest.starts_with("")
                    && rest.contains("iryna@ministry.example"),
                "{lang}: {body}"
            );
            assert!(body.contains("https://ortus.cc/welcome/accept#t"), "{lang}");
            let (_, plain) = invitation(lang, "Taras", "https://ortus.cc/welcome/accept#t", None);
            assert!(!plain.contains("iryna@"), "{lang}");
        }
    }

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
