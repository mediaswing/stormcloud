# Contributing

Thank you for your interest in Graphical Cloud Management. Contributions are
welcome, but please read this first.

## A personal project

This is a personal project that I maintain in my spare time. I read issues and
pull requests when I can, but I can't promise to reply to them, review them or
merge them, or to do any of that quickly. Please don't take silence as a
judgement on your work.

If you'd like to make a large change, such as a new node in the console tree,
open an issue to describe it before you write the code. That way you won't
spend time on something that doesn't fit the project.

## Security issues

Please don't report security problems in a public issue. Use GitHub's
[private vulnerability reporting](https://github.com/mediaswing/stormcloud/security/advisories/new)
instead, from the **Security** tab of the repository. Only the maintainer can
see a private report.

## Bug reports

A good bug report says:

- which version of the app you're using (**Help › About Graphical Cloud
  Management**), and on which operating system
- which screen reader you use, if any, and its version
- what you did, what you expected, and what happened instead
- the Entra roles you were signed in with, if the problem is with changing
  something

The log (`output.log`, in the same folder as `settings.ini`) often helps. Set
`level = debug` in the `[logging]` section of `settings.ini`, restart the app and
do what went wrong again, so it records every request. Once it reaches 5 MB it's
renamed to `output.log.1`, so the lines you need may be in that file instead. It never records passwords, client secrets or tokens, but
it does contain user names, group names, request bodies and your tenant ID, so
please check it and remove anything you don't want to make public before
attaching it.

## Design principles

Please keep these in mind for any change. A change that goes against them is
unlikely to be merged, however useful it is.

### 1. Accessibility comes first

The app must work for administrators who rely on a keyboard, a screen reader or
both, so every feature must work for them from the start, not be added later.

- **Everything works from the keyboard.** Every control can be reached with
  Tab and Shift+Tab, the panes with F6 and Shift+F6, and every action has a
  menu item, a context menu entry or a shortcut. New shortcuts must not clash
  with common screen reader keys, and they belong in the keyboard table in the
  README and in the F1 help.
- **Everything is exposed to screen readers.** Each control has a visible label
  that's linked to it (`labelled_by`), panes have headings (`pane_heading` in
  `src/app.rs`), and list rows announce every column.
- **It looks and works like a console.** New features fit the layout people
  already know: the console tree, the results list, the Actions pane and
  property sheets with tabs.
- **The user is kept informed.** Progress, results and errors are reported in
  the status bar or a dialog, in words that make sense when spoken aloud. When
  something fails because of a missing role, licence or permission, say which
  one is needed.

If you can, try your change with a screen reader (Narrator or NVDA on Windows,
VoiceOver on macOS, Orca on Linux) and say in the pull request which one you
used.

### 2. Careful with the tenant

This app changes a real organisation's directory, so mistakes matter.

- **Ask before changing anything that's hard to undo.** Deleting, disabling,
  removing access or revoking sessions goes through a confirmation
  (`Dialog::confirm`) that says what will happen and what stops working.
- **Show the result of every step.** When an action makes several changes, as
  the onboarding and offboarding wizards do, report each one's result, and don't
  carry on with a step that depends on one that failed (see `src/workflow.rs`).
- **Ask for as little as possible.** Use the narrowest Microsoft Graph
  permission that does the job. A permission that only some features need is
  asked for the first time one of them is used, as the Authentication methods
  and Sign-ins tabs do, not added to the default scopes. Describe any new
  permission in the README's Setup section.
- **Test against a test tenant**, never a production one.

### 3. Privacy and secrets

- Passwords, client secrets, tokens and Temporary Access Passes are never
  written to the debug log. Nor are phone numbers or sign-in details.
- The app only talks to the sign-in, Graph and Exchange endpoints in
  `settings.ini`, to `api.github.com` to check for updates, to a MariaDB
  server if the user has set one up, and to Ollama on the same computer
  (`127.0.0.1:11434`) when the user translates the app. Every network request must be something
  the user can understand and, where it isn't essential, turn off. Describe any
  new one in the README.
- Phone numbers and email addresses used for sign-in are masked when they're
  shown.

### 4. The window never freezes

Anything slow, such as a Graph or Exchange request or reading a file, runs on a
background thread (`App::spawn` in `src/app.rs`) and reports back to the window
through a message. A frozen window looks like a crash to someone using a screen
reader.

### 5. Each platform is self-contained

macOS and Linux code lives in `src/platform/`, one file per operating system.
The rest of the app shouldn't need to know which platform it's running on. The
app packages itself (`--package-macos`, `--package-deb`), so please don't add
shell scripts for building or packaging. The one exception is the Windows
installer: WiX Toolset builds it from `src/installer/stormcloud.wxs` in the
release workflow, from the files the workflow stages. If you change which files
the Windows app needs, update both.

### 6. Plain language

Status messages, labels, the README and the changelog are written for the
people who use the app, in plain British English. Prefer short sentences and
everyday words, and use Microsoft's own names for things in Entra ID and
Exchange Online. The same goes for code comments.

Every label and message the app shows lives in the `ENGLISH` list in
`src/i18n.rs`, under a key, so that it can be translated. Use `t("key")` or
`tf("key", &[("name", &value)])` rather than writing the text in place, and
write values as `{name}` rather than building sentences from pieces, since
word order differs between languages. Tables of labels, such as column titles,
hold keys and look them up when they're shown. Tests check that every key in
the code has English text and that every English text is used. Log messages
stay in English, and so does anything exported to a file or database that has
to read the same each time: wrap it in `i18n::in_english`. Never decide
anything from translated text; keep a flag or the key instead.

### 7. Few dependencies

Add a new crate only when it's clearly worth it, and turn off the features you
don't need. Prefer crates that use rustls rather than OpenSSL. Smaller
dependency trees build faster on all three platforms and are easier to keep
secure.

## Using AI tools

You're welcome to use AI tools to help you write code, tests or
documentation. Parts of this project were written with them. But you are
responsible for what you submit, however it was made, so before you open a
pull request:

- **Make sure it works.** Build it, run the tests, and then use the app to try
  the change yourself against a test tenant. Passing tests aren't enough on
  their own: check that it does what you expect, from the keyboard and, if you
  can, with a screen reader.
- **Read every line.** You should understand the change well enough to explain
  it and answer questions about it. If you can't say why a line is there, find
  out or take it out.
- **Check the claims.** AI tools can invent functions, crate features, Graph
  endpoints, permissions and facts that sound right but aren't. Check anything
  you didn't already know against Microsoft's documentation, including the
  wording of README and changelog entries.
- **Keep it focused.** Submit the change you meant to make, not everything
  the tool tidied up along the way. Large, unreviewed rewrites won't be
  merged.
- **Say so.** Mention in the pull request that you used an AI tool, and
  describe how you tested the result.

The same goes for bug and security reports: please make sure what you report
really happens before you send it.

## Making a change

1. Fork the repository and create a branch for your change.
2. Build and test it:

   ```
   cargo build
   cargo test
   cargo clippy
   ```

   See the README for the Linux build dependencies.
3. Add tests for new behaviour where you can, especially in code that doesn't
   need a tenant, such as CSV handling, parsing Graph responses and working out
   which changes a wizard will make.
4. If the change affects people who use the app, add a line to the
   `[Unreleased]` section of `CHANGELOG.md`. Write it for them, not for
   developers.
5. Update the README if you've changed what the app does, its keyboard
   shortcuts, the permissions or roles it needs, or what it sends over the
   network.
6. Open a pull request that explains what the change does and why, and how you
   tested it.

Keep each pull request to one change. Small pull requests are much easier to
review.

## Licence

Graphical Cloud Management is released under the
[GNU General Public License, version 3 or later](LICENSE). By contributing, you
agree that your contribution will be released under the same licence.
