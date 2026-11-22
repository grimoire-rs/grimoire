// Sample mod for the manual rig. Only the server entry is served, and the
// server side has no toast, so the visible effect is a command and a shell variable.
import { Plugin } from "@opencode/plugin";

const HELLO = "hello from the grim hello-mod";

export default Plugin.define({
  id: "grim-hello-mod",
  async setup(ctx) {
    await ctx.command.transform((commands) => {
      commands.add({
        name: "grim-hello",
        description: "Answers with a fixed line from the hello-mod",
        execute: async ({ sessionID }) => {
          await ctx.session.synthetic({ sessionID, text: HELLO, resume: false });
        },
      });
    });
    await ctx.shell.hook("create.before", (shell) => {
      shell.env.GRIM_HELLO_MOD = HELLO;
    });
  },
});
