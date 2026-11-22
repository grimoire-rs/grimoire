// Sample mod for the manual rig: three visible effects, plain calls only.
export function register(on) {
  on("session.start", async ($, e, next) => {
    await $.ui.status("grim hello-mod loaded");
    await $.ui.toast("hello from the grim hello-mod");
    await $.command.register({
      name: "grim-hello",
      description: "Answers with a fixed line from the hello-mod",
    });
    return next(e);
  });

  on("command.run", { command: "grim-hello" }, async () => {
    return { text: "hello from the grim hello-mod" };
  });
}
