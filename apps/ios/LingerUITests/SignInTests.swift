import XCTest

/// Signs in through Clerk's own sheet on the development instance as its existing test user, whose
/// email code is 424242 without any mail sent (server/scripts/clerk-test-token.ts signs in the
/// same user), and lands in the reader. Starts signed out: on a simulator that has passed it
/// before, run `xcrun simctl keychain <device> reset` (or `erase`) first.
final class SignInTests: XCTestCase {
    func testSigningInOpensTheReader() {
        let app = XCUIApplication()
        app.launch()
        app.buttons["Sign in"].tap()

        // Until it has focus, an empty field shows only as its label, so match the identifier on any
        // element; once focused it is a text field, holding the address Clerk remembers, if any.
        let address = "amp+clerk_test@example.com"
        let email = app.descendants(matching: .any)["clerk.auth.start.identifier"].firstMatch
        XCTAssertTrue(email.waitForExistence(timeout: 15), "Clerk's sign-in sheet didn't open")
        email.tap()
        let field = app.textFields["clerk.auth.start.identifier"]
        XCTAssertTrue(field.waitForExistence(timeout: 5), "the email field didn't take focus")
        let remembered = (field.value as? String) ?? ""
        if remembered != address {
            app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: remembered.count) + address)
        }
        app.buttons["clerk.auth.start.continue"].tap()

        // The development instance offers this user's password first; take the email code.
        let other = app.buttons["clerk.auth.signIn.useAnotherMethod"]
        XCTAssertTrue(other.waitForExistence(timeout: 15))
        other.tap()
        app.buttons["clerk.auth.signIn.alternativeMethod.email_code"].tap()

        XCTAssertTrue(app.staticTexts["Check your email"].waitForExistence(timeout: 15))
        // The code field has focus.
        app.typeText("424242")

        // The library consent sheet, unless this device already answered it.
        let consent = app.buttons["Turn On Sync"]
        if consent.waitForExistence(timeout: 10) { consent.tap() }
        XCTAssertTrue(app.searchFields["Go to a reference"].waitForExistence(timeout: 20), "the reader didn't appear")
    }
}
