//! Invitations an administrator can always pass on: the link (and its QR
//! code) in every answer, the letter to a personal address, none to a
//! mailbox here that its person cannot read yet, a new link or the
//! letter again, withdrawing, and the links of a CSV import.

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::config::Config;
use ortus_core::instance::{testing, testing_auth::signed_in, Instance};
use ortus_core::interfaces::MailDomains;
use ortus_core::runtime::block_on;
use ortus_core::storage::BoxFuture;
use rust_web_server::application::Application;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};

/// The mail app's answer: ministry.example is a mail domain here.
struct Ours;

impl MailDomains for Ours {
    fn is_ours<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { domain == "ministry.example" })
    }
}

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

fn send<A: Application>(
    client: &TestClient<A>,
    method: &str,
    path: &str,
    token: &str,
    body: &str,
) -> TestResponse {
    let request = match method {
        "GET" => client.get(path),
        "POST" => client.post(path),
        _ => client.delete(path),
    };
    request
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus")
        .body_text(body)
        .send()
}

/// The addresses letters were queued for.
fn letters_to(instance: &Arc<Instance>) -> Vec<String> {
    block_on(instance.store.query(
        "SELECT payload FROM ortus_jobs WHERE kind = 'core.mail' ORDER BY created_at",
        &[],
    ))
    .unwrap()
    .iter()
    .filter_map(|r| serde_json::from_str::<Value>(r.text(0)?).ok())
    .filter_map(|v| v["to"].as_str().map(str::to_string))
    .collect()
}

fn installation(mail: bool) -> Arc<Instance> {
    let env = move |key: &str| match key {
        "ORTUS_MAIL_HOST" if mail => Some("127.0.0.1".to_string()),
        "ORTUS_MAIL_FROM" if mail => Some("Ortus <noreply@ministry.example>".to_string()),
        _ => None,
    };
    let instance = testing::instance_with(Config::load(None, &env).unwrap(), Vec::new());
    instance
        .services
        .provide::<dyn MailDomains>("mail", Arc::new(Ours));
    instance
}

#[test]
fn an_invitation_reaches_its_person() {
    let instance = installation(true);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let admin = signed_in(&instance, "taras@ministry.example", Role::Admin);

    // An address outside: the letter goes there; the link is in the answer.
    let outside = json_of(&send(
        &client,
        "POST",
        "/api/v1/admin/users",
        &admin,
        &json!({"email": "anna@regional.example", "name": "Anna Kowalska"}).to_string(),
    ));
    assert_eq!("anna@regional.example", outside["sent_to"], "{outside}");
    assert!(outside["not_sent"].is_null());
    assert!(outside["link"]
        .as_str()
        .unwrap()
        .contains("/welcome/accept#"));
    assert!(
        outside["qr_svg"].as_str().unwrap().starts_with("<?xml")
            || outside["qr_svg"].as_str().unwrap().contains("<svg")
    );
    assert!(outside["expires_at"].as_i64().unwrap() > ortus_core::program::now_ms());
    assert_eq!(
        outside["link"].is_string(),
        outside["invitation"]["expires_at"].is_i64()
    );

    // A mailbox here, no personal address: no letter (it would wait in a
    // mailbox its person cannot open); the administrator passes on the link.
    let own = json_of(&send(
        &client,
        "POST",
        "/api/v1/admin/users",
        &admin,
        &json!({"email": "bohdan@ministry.example", "name": "Bohdan Tsap"}).to_string(),
    ));
    assert!(own["sent_to"].is_null(), "{own}");
    assert_eq!("own_mailbox", own["not_sent"]);
    assert!(own["link"].is_string());

    // A mailbox here with a personal address: the letter goes there and
    // names the account.
    let personal = json_of(&send(&client, "POST", "/api/v1/admin/users", &admin, &json!({"email": "iryna@ministry.example", "name": "Iryna Melnyk", "send_to": " Iryna.Melnyk@Gmail.example "}).to_string()));
    assert_eq!(
        "iryna.melnyk@gmail.example", personal["sent_to"],
        "{personal}"
    );
    assert_eq!(
        "iryna.melnyk@gmail.example",
        personal["invitation"]["send_to"]
    );
    let letter = block_on(instance.store.query(
        "SELECT payload FROM ortus_jobs WHERE kind = 'core.mail' AND payload LIKE '%gmail.example%'",
        &[],
    ))
    .unwrap();
    let text = letter.first().and_then(|r| r.text(0)).unwrap().to_string();
    assert!(
        text.contains("iryna@ministry.example"),
        "the letter names the account: {text}"
    );
    assert_eq!(
        vec!["anna@regional.example", "iryna.melnyk@gmail.example"],
        letters_to(&instance)
    );
    assert_eq!(
        400,
        send(&client, "POST", "/api/v1/admin/users", &admin, &json!({"email": "olena@ministry.example", "name": "Olena", "send_to": "not an address"}).to_string()).status()
    );

    // The listing shows each invitation and where its letter went.
    let list = json_of(&send(&client, "GET", "/api/v1/admin/users", &admin, ""));
    let iryna = list["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == "iryna@ministry.example")
        .unwrap()
        .clone();
    assert_eq!(
        "iryna.melnyk@gmail.example", iryna["invitation"]["send_to"],
        "{iryna}"
    );
    let bohdan_id = own["id"].as_str().unwrap().to_string();
    let iryna_id = iryna["id"].as_str().unwrap().to_string();

    // A new link only: the old one stops working, nothing is sent.
    let old = own["link"]
        .as_str()
        .unwrap()
        .rsplit_once('#')
        .unwrap()
        .1
        .to_string();
    let renewed = json_of(&send(
        &client,
        "POST",
        &format!("/api/v1/admin/users/{bohdan_id}/invitation"),
        &admin,
        &json!({"send": false}).to_string(),
    ));
    assert_eq!("not_asked", renewed["not_sent"], "{renewed}");
    assert_ne!(own["link"], renewed["link"]);
    let anyone = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let accept = |token: &str| {
        anyone
            .post("/api/v1/invitation/accept")
            .header("X-Requested-With", "ortus")
            .body_text(&json!({ "token": token }).to_string())
            .send()
    };
    assert_eq!(410, accept(&old).status(), "the old link");

    // The letter again: to where the last one went.
    let again = json_of(&send(
        &client,
        "POST",
        &format!("/api/v1/admin/users/{iryna_id}/invitation"),
        &admin,
        "",
    ));
    assert_eq!("iryna.melnyk@gmail.example", again["sent_to"], "{again}");
    // Or to a personal address given now.
    let bohdan = json_of(&send(
        &client,
        "POST",
        &format!("/api/v1/admin/users/{bohdan_id}/invitation"),
        &admin,
        &json!({"send_to": "bohdan@gmail.example"}).to_string(),
    ));
    assert_eq!("bohdan@gmail.example", bohdan["sent_to"], "{bohdan}");

    // Withdrawn: the link stops working; still listed, without a link.
    let token = bohdan["link"]
        .as_str()
        .unwrap()
        .rsplit_once('#')
        .unwrap()
        .1
        .to_string();
    assert_eq!(
        200,
        send(
            &client,
            "DELETE",
            &format!("/api/v1/admin/users/{bohdan_id}/invitation"),
            &admin,
            ""
        )
        .status()
    );
    assert_eq!(410, accept(&token).status(), "a withdrawn link");
    let list = json_of(&send(
        &client,
        "GET",
        "/api/v1/admin/users?q=bohdan",
        &admin,
        "",
    ));
    assert!(list["users"][0]["invitation"].is_null(), "{list}");
    assert_eq!("invited", list["users"][0]["state"]);

    // Someone who joined has no invitation to renew.
    let latest = json_of(&send(
        &client,
        "POST",
        &format!("/api/v1/admin/users/{iryna_id}/invitation"),
        &admin,
        &json!({"send": false}).to_string(),
    ));
    let token = latest["link"]
        .as_str()
        .unwrap()
        .rsplit_once('#')
        .unwrap()
        .1
        .to_string();
    assert_eq!(200, accept(&token).status());
    let joined = send(
        &client,
        "POST",
        &format!("/api/v1/admin/users/{iryna_id}/invitation"),
        &admin,
        "",
    );
    assert_eq!(409, joined.status());
    assert_eq!("not_invited", json_of(&joined)["error"]["code"]);

    // A member may do none of this.
    let member = signed_in(&instance, "ana@regional.example", Role::Member);
    assert_eq!(
        403,
        send(
            &client,
            "POST",
            &format!("/api/v1/admin/users/{bohdan_id}/invitation"),
            &member,
            ""
        )
        .status()
    );
    assert_eq!(
        403,
        send(
            &client,
            "DELETE",
            &format!("/api/v1/admin/users/{bohdan_id}/invitation"),
            &member,
            ""
        )
        .status()
    );
}

#[test]
fn a_csv_import_lists_every_link() {
    let instance = installation(true);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let admin = signed_in(&instance, "taras@ministry.example", Role::Admin);
    let csv = "email,name,role,groups,personal email\n\
               marek@ministry.example,Marek Nowak,member,Logistics,marek@gmail.example\n\
               olena@ministry.example,Olena Hrytsenko\n\
               anna@regional.example,Anna Kowalska,,,\n\
               broken line\n";
    let answer = json_of(&send(
        &client,
        "POST",
        "/api/v1/admin/users/import",
        &admin,
        csv,
    ));
    let invited = answer["invited"].as_array().unwrap();
    assert_eq!(3, invited.len(), "{answer}");
    assert_eq!(1, answer["failed"].as_array().unwrap().len());
    let by = |email: &str| {
        invited
            .iter()
            .find(|i| i["email"] == email)
            .unwrap()
            .clone()
    };
    assert_eq!(
        "marek@gmail.example",
        by("marek@ministry.example")["sent_to"]
    );
    assert_eq!("own_mailbox", by("olena@ministry.example")["not_sent"]);
    assert_eq!(
        "anna@regional.example",
        by("anna@regional.example")["sent_to"]
    );
    assert!(invited
        .iter()
        .all(|i| i["link"].as_str().unwrap().contains("/welcome/accept#")));
}

#[test]
fn without_outgoing_mail_the_link_is_the_way() {
    let instance = installation(false);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let admin = signed_in(&instance, "taras@ministry.example", Role::Admin);
    let answer = json_of(&send(&client, "POST", "/api/v1/admin/users", &admin, &json!({"email": "anna@regional.example", "name": "Anna Kowalska", "send_to": "anna@gmail.example"}).to_string()));
    assert_eq!("no_mail", answer["not_sent"], "{answer}");
    assert!(answer["link"].is_string());
    assert!(letters_to(&instance).is_empty());
}
