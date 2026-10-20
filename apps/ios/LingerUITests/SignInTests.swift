import XCTest

/// Signs in through Clerk's own sheet on the development instance as its existing test user, whose
/// email code is 424242 without any mail sent (server/scripts/clerk-test-token.ts signs in the
/// same user), and lands in the reader. Starts signed out: on a simulator that has passed it
/// before, run `xcrun simctl keychain <device> reset` first.
final class SignInTests: XCTestCase {
    func testSigningInOpensTheReader() {
        let app = XCUIApplication()
        app.launch()
        app.buttons["Sign in"].tap()

        let email = app.textFields["clerk.auth.start.identifier"]
        XCTAssertTrue(email.waitForExistence(timeout: 15), "Clerk's sign-in sheet didn't open")
        // Clerk fills in the last address used on this device: select it all, so typing replaces it.
        email.tap(withNumberOfTaps: 3, numberOfTouches: 1)
        email.typeText("amp+clerk_test@example.com")
        app.buttons["clerk.auth.start.continue"].tap()

        // The development instance offers this user's password first; take the email code.
        let other = app.buttons["clerk.auth.signIn.useAnotherMethod"]
        XCTAssertTrue(other.waitForExistence(timeout: 15))
        other.tap()
        app.buttons["clerk.auth.signIn.alternativeMethod.email_code"].tap()

        XCTAssertTrue(app.staticTexts["Check your email"].waitForExistence(timeout: 15))
        // The code field has focus.
        app.typeText("424242")

        XCTAssertTrue(app.searchFields["Go to a reference"].waitForExistence(timeout: 20), "the reader didn't appear")
    }
}
