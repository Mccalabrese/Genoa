return {
  {
    "saghen/blink.cmp",
    opts = {
      keymap = {
        ["<CR>"] = { "fallback" }, -- Enter: just newline
        ["<Tab>"] = { "fallback" }, -- Tab: just tab/indent
        ["<S-Tab>"] = { "fallback" }, -- Shift-Tab: normal behaviour
        ["<M-Right>"] = { "accept" }, -- Alt+Right: accept the selected completion
      },
      completion = {
        menu = {
          auto_show = true, -- show the menu automatically when typing
        },
        ghost_text = {
          enabled = false, -- disable ghost text
        },
        list = {
          selection = {
            auto_insert = false, -- ensure completion isn’t auto-inserted
          },
        },
      },
      sources = {
        default = { "lsp", "path", "buffer", "snippets" },
      },
    },
  },
}
