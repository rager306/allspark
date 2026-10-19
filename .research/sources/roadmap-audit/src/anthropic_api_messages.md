url: https://platform.claude.com/docs/en/api/messages
title: Messages - Claude API Reference
content: >-
  Claude Platform Docs

  Messages

  Managed Agents

  Admin

  Resources

  

  

  API reference

  

  English

  

  

  Console

  Log in

  

  Search

  Ctrl

  K

  

  Include beta APIs

  Using the API

  Features overview

  Beta headers

  Errors

  Messages

  

  Create a Message

  

  Count tokens in a Message

  Batches

  

  Managed Agents

  

  Agents

  

  Environments

  

  Sessions

  

  Deployments

  

  Deployment Runs

  

  Vaults

  

  Memory Stores

  

  Dreams

  

  Files

  

  Upload File

  

  List Files

  

  Download File

  

  Get File Metadata

  

  Delete File

  Models

  

  List Models

  

  Get a Model

  Organization

  

  Get Current Organization

  API Keys

  

  External Keys

  

  Federation

  

  Invites

  

  Service Accounts

  

  Users

  

  Workspaces

  

  Rate Limits

  

  Compliance Settings

  

  Usage Report

  

  Cost Report

  

  MCP Tunnels

  

  

  Analytics

  

  Spend Limits

  

  RBAC Groups

  

  RBAC Roles

  

  Plugins

  

  

  Plugin Marketplaces

  

  

  Telemetry Destinations

  

  

  Skills

  

  Create Skill

  

  List Skills

  

  Get Skill

  

  Delete Skill

  Versions

  

  Tunnels

  

  

  Create Tunnel

  

  Get Tunnel

  

  List Tunnels

  

  Archive Tunnel

  

  Reveal Tunnel Token

  

  Rotate Tunnel Token

  Certificates

  

  User Profiles

  

  

  Create User Profile

  

  List User Profiles

  

  Get User Profile

  

  Update User Profile

  

  Create Enrollment URL

  Compliance API

  Activities

  

  Organizations

  

  Groups

  

  Apps

  

  Completions

  

  Create a Text Completion

  Deprecated

  OpenTelemetry

  Spans

  

  Support & configuration

  Rate limits

  Service tiers

  IAM actions (Claude Platform on AWS)

  Versions

  IP addresses

  Supported regions

  Claude Code

  Trigger a routine

  

  

  A beta version of this API exists and may have additional functionality. View
  the beta version.

  API reference

  Messages

  

  Copy page

  

  cURL

  

  Create a Message

  

  POST

  /v1/messages


  Send a structured list of input messages with text and/or image content, and
  the model will generate the next message in the conversation.


  Count tokens in a Message

  

  POST

  /v1/messages/count_tokens


  Count the number of tokens in a Message.


  Models

  

  Base64ImageSource { type: "base64", data, media_type }

  

  

  Base64PDFSource { type: "base64", data, media_type }

  

  

  BashCodeExecutionOutputBlock { type: "bash_code_execution_output", file_id }

  

  

  BashCodeExecutionOutputBlockParam { type: "bash_code_execution_output",
  file_id }

  

  

  BashCodeExecutionResultBlock { type: "bash_code_execution_result", content,
  return_code, 2 more }

  

  

  BashCodeExecutionResultBlockParam { type: "bash_code_execution_result",
  content, return_code, 2 more }

  

  

  BashCodeExecutionToolResultBlock { type: "bash_code_execution_tool_result",
  content, tool_use_id }

  

  

  BashCodeExecutionToolResultBlockParam { type:
  "bash_code_execution_tool_result", content, tool_use_id, cache_control }

  

  

  BashCodeExecutionToolResultError { type:
  "bash_code_execution_tool_result_error", error_code }

  

  

  BashCodeExecutionToolResultErrorCode = "invalid_tool_input" or "unavailable"
  or "too_many_requests" or 2 more

  

  

  BashCodeExecutionToolResultErrorParam { type:
  "bash_code_execution_tool_result_error", error_code }

  

  

  BrowserClickTarget = BrowserCoordinateTarget or BrowserRefTarget

  


  Where to act: either a viewport coordinate or an element reference.


  

  BrowserCloseTabConfig { defer_loading, enabled }

  


  close_tab's config overrides.


  

  BrowserCloseTabInput { tab_id }

  


  Close the tab with the given tab_id.


  

  BrowserCloseTabToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserCoordinateTarget { type: "coordinate", x, y }

  


  A point in the browser viewport, in viewport pixels (the same frame as a
  full-viewport screenshot).


  

  BrowserDoubleClickConfig { defer_loading, enabled }

  


  double_click's config overrides.


  

  BrowserDoubleClickInput { target, modifiers, tab_id }

  


  Double left-click at a viewport coordinate or on an element by reference.


  

  BrowserDoubleClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserFileUploadConfig { defer_loading, enabled }

  


  file_upload's config overrides.


  

  BrowserFileUploadInput { target, document_ids, paths, tab_id }

  


  Set the value of a file-input element to one or more files. The target must be
  an element reference; at least one of paths or document_ids is required.


  

  BrowserFileUploadToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserFindConfig { defer_loading, enabled }

  


  find's config overrides.


  

  BrowserFindInput { query, tab_id }

  


  Find elements matching a natural-language description (e.g. "search bar", "add
  to cart button") and return up to 20 matches with element references.


  

  BrowserFindToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserFormInputConfig { defer_loading, enabled }

  


  form_input's config overrides.


  

  BrowserFormInputInput { target, value, tab_id }

  


  Set the value of a form element (input, textarea, select, checkbox). Use a
  boolean for checkboxes, an option value or text for selects.


  

  BrowserFormInputToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserFormInputValue = string or number or boolean

  

  

  BrowserGetPageTextConfig { defer_loading, enabled }

  


  get_page_text's config overrides.


  

  BrowserGetPageTextInput { tab_id }

  


  Return the page's visible text content as plain text, prioritizing article
  content. Suited to articles, documentation, and other text-heavy pages.


  

  BrowserGetPageTextToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserHoldKeyConfig { defer_loading, enabled }

  


  hold_key's config overrides.


  

  BrowserHoldKeyInput { duration, text, tab_id }

  


  Hold a key or key chord down for a duration, then release it. Uses the same
  key names and "+" chord syntax as the key action.


  

  BrowserHoldKeyToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserHoverConfig { defer_loading, enabled }

  


  hover's config overrides.


  

  BrowserHoverInput { target, tab_id }

  


  Move the cursor to a coordinate or element without clicking.


  

  BrowserHoverToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserJavascriptExecConfig { defer_loading, enabled }

  


  javascript_exec's config overrides.


  

  BrowserJavascriptExecInput { text, tab_id }

  


  Execute JavaScript in the page context and return the value of the last
  expression. The code runs with access to the DOM, window, and page variables.
  Write the expression you want evaluated — do NOT use return.


  

  BrowserJavascriptExecToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserKeyConfig { defer_loading, enabled }

  


  key's config overrides.


  

  BrowserKeyInput { text, repeat, tab_id }

  


  Press a key or key chord. Use "+" to combine modifiers with a key (e.g.
  "ctrl+a", "cmd+shift+p") and space to sequence presses (e.g. "Backspace
  Backspace Delete"). Common names like "Return", "Tab", "Escape", "BackSpace"
  are supported.


  

  BrowserKeyToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserLeftClickConfig { defer_loading, enabled }

  


  left_click's config overrides.


  

  BrowserLeftClickDragConfig { defer_loading, enabled }

  


  left_click_drag's config overrides.


  

  BrowserLeftClickDragInput { from, target, tab_id }

  


  Press at from, drag to target, release. Both must be coordinate targets.


  

  BrowserLeftClickDragToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserLeftClickInput { target, modifiers, tab_id }

  


  Left-click at a viewport coordinate or on an element by reference.


  

  BrowserLeftClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserLeftMouseDownConfig { defer_loading, enabled }

  


  left_mouse_down's config overrides.


  

  BrowserLeftMouseDownInput { target, tab_id }

  


  Press and hold the left mouse button at a viewport coordinate. Pair with
  left_mouse_up to perform a custom drag.


  

  BrowserLeftMouseDownToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserLeftMouseUpConfig { defer_loading, enabled }

  


  left_mouse_up's config overrides.


  

  BrowserLeftMouseUpInput { target, tab_id }

  


  Release the left mouse button at a viewport coordinate.


  

  BrowserLeftMouseUpToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserListTabsConfig { defer_loading, enabled }

  


  list_tabs's config overrides.


  BrowserListTabsInput object{  }

  


  List all open tabs with each tab's tab_id, title, and URL.


  

  BrowserListTabsToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserMiddleClickConfig { defer_loading, enabled }

  


  middle_click's config overrides.


  

  BrowserMiddleClickInput { target, modifiers, tab_id }

  


  Middle-click at a viewport coordinate or on an element by reference.


  

  BrowserMiddleClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserMouseMoveConfig { defer_loading, enabled }

  


  mouse_move's config overrides.


  

  BrowserMouseMoveInput { target, tab_id }

  


  Move the pointer to a viewport coordinate without clicking.


  

  BrowserMouseMoveToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserNavigateConfig { defer_loading, enabled }

  


  navigate's config overrides.


  

  BrowserNavigateInput { url, tab_id }

  


  Navigate to a URL, or go back/forward/reload in history. The protocol may be
  omitted (defaults to https://).


  

  BrowserNavigateToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserNewTabConfig { defer_loading, enabled }

  


  new_tab's config overrides.


  BrowserNewTabInput object{  }

  


  Open a new empty tab and return its tab_id.


  

  BrowserNewTabToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserReadConsoleConfig { defer_loading, enabled }

  


  read_console's config overrides.


  

  BrowserReadConsoleInput { tab_id }

  


  Return console output (log entries, errors, warnings) accumulated since the
  driver attached to the tab and since the last read, one line per entry. An
  empty result does not mean no traffic for a tab that predates attach.


  

  BrowserReadConsoleToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserReadNetworkConfig { defer_loading, enabled }

  


  read_network's config overrides.


  

  BrowserReadNetworkInput { tab_id }

  


  Return the network requests (method, URL, status, MIME type, timing) recorded
  since the driver attached to the tab and since the last read, one line per
  entry. An empty result does not mean no traffic for a tab that predates
  attach.


  

  BrowserReadNetworkToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserReadPageConfig { defer_loading, enabled }

  


  read_page's config overrides.


  

  BrowserReadPageFilter = "all" or "interactive"

  

  

  BrowserReadPageInput { depth, filter, ref, tab_id }

  


  Return a structured accessibility tree of the page (or the subtree rooted at
  ref), with element references like [ref_7] that can be used as targets on
  later actions. Output is capped at 50,000 characters — narrow with ref or a
  smaller depth when exceeded.


  

  BrowserReadPageToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserRefTarget { type: "ref", ref }

  


  An element on the page, identified by a reference from a prior read_page or
  find result. References are scoped to the tab that produced them and become
  stale after navigation or a major re-render.


  

  BrowserRightClickConfig { defer_loading, enabled }

  


  right_click's config overrides.


  

  BrowserRightClickInput { target, modifiers, tab_id }

  


  Right-click at a viewport coordinate or on an element by reference.


  

  BrowserRightClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserScreenshotConfig { defer_loading, enabled }

  


  screenshot's config overrides.


  

  BrowserScreenshotInput { tab_id }

  


  Capture the current browser viewport.


  

  BrowserScreenshotToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserScrollConfig { defer_loading, enabled }

  


  scroll's config overrides.


  

  BrowserScrollDirection = "up" or "down" or "left" or "right"

  

  

  BrowserScrollInput { scroll_direction, target, scroll_amount, tab_id }

  


  Scroll at a viewport position. target must be a coordinate target.


  

  BrowserScrollToConfig { defer_loading, enabled }

  


  scroll_to's config overrides.


  

  BrowserScrollToInput { target, tab_id }

  


  Scroll an element into view.


  

  BrowserScrollToToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserScrollToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserStateBlockParam { type: "browser_state", tabs, cache_control,
  state_changes }

  


  The caller's browser state after a browser toolset member call — the full
  inventory of open tabs, which tab is active, and any side effects (tabs
  opened, download state changes) the call produced.


  

  BrowserStateChange = BrowserStateChangeTabOpened or
  BrowserStateChangeDownloadStarted or BrowserStateChangeDownloadCompleted or
  BrowserStateChangeDownloadFailed

  

  

  BrowserStateChangeDownloadCompleted { type: "download_completed", download_id,
  url, 2 more }

  


  A file download that finished during this call, reported with the same
  download_id as its download_started — or without a prior download_started,
  when the download finished during the call that started it (at most one state
  change per download_id per result).


  

  BrowserStateChangeDownloadFailed { type: "download_failed", download_id, url,
  error }

  


  A file download that failed — or was cancelled — during this call.


  

  BrowserStateChangeDownloadStarted { type: "download_started", download_id,
  url }

  


  A file download that started during this call.


  

  BrowserStateChangeTabOpened { type: "tab_opened", tab_id }

  


  A tab this call's execution opened that remains open at its end — the creation
  delta of the tabs inventory, not an event log.


  

  BrowserStateTabEntry { tab_id, title, url, active }

  


  One open browser tab reported in a browser_state block's tabs inventory.


  

  BrowserSwitchTabConfig { defer_loading, enabled }

  


  switch_tab's config overrides.


  

  BrowserSwitchTabInput { tab_id }

  


  Make the tab with the given tab_id the active tab — the tab that actions
  without a tab_id apply to.


  

  BrowserSwitchTabToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserToolUseBlock = BrowserNavigateToolUseBlock or
  BrowserListTabsToolUseBlock or BrowserNewTabToolUseBlock or 28 more

  

  

  BrowserToolset20260801 { type: "browser_toolset_20260801", cache_control,
  configs }

  


  The browser toolset: a single tools[] entry (carrying no name) that declares
  the browser tool family. The model is served the family's tool with any
  members disabled via configs removed from its schema.


  

  BrowserToolsetConfigs { type, close_tab, double_click, 28 more }

  


  Per-member configuration for browser_toolset_20260801: one optional field per
  member tool, keyed by the member name — the same name the member's tool_use
  blocks carry. Every member is an accepted key, and a member's defaults apply
  wherever its key is absent. Unknown keys are rejected: the field set is this
  toolset version's complete member set.


  

  BrowserTripleClickConfig { defer_loading, enabled }

  


  triple_click's config overrides.


  

  BrowserTripleClickInput { target, modifiers, tab_id }

  


  Triple left-click at a viewport coordinate or on an element by reference
  (typically selects a line or paragraph).


  

  BrowserTripleClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserTypeConfig { defer_loading, enabled }

  


  type's config overrides.


  

  BrowserTypeInput { text, tab_id }

  


  Type a literal string at the current focus.


  

  BrowserTypeToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserWaitConfig { defer_loading, enabled }

  


  wait's config overrides.


  

  BrowserWaitInput { duration, tab_id }

  


  Pause for the given duration.


  

  BrowserWaitToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  BrowserZoomConfig { defer_loading, enabled }

  


  zoom's config overrides.


  

  BrowserZoomInput { region, tab_id }

  


  Return a cropped screenshot of the given viewport region, scaled up for closer
  inspection — useful for small icons, buttons, or text. Coordinates are in the
  same viewport-pixel space as a full screenshot.


  

  BrowserZoomToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  CacheControlEphemeral { type: "ephemeral", ttl }

  

  

  CacheCreation { ephemeral_1h_input_tokens, ephemeral_5m_input_tokens }

  

  

  CacheMissMessagesChanged { type: "messages_changed",
  cache_missed_input_tokens }

  

  

  CacheMissModelChanged { type: "model_changed", cache_missed_input_tokens }

  

  

  CacheMissPreviousMessageNotFound { type: "previous_message_not_found" }

  

  

  CacheMissReason = CacheMissModelChanged or CacheMissSystemChanged or
  CacheMissToolsChanged or 3 more

  

  

  CacheMissSystemChanged { type: "system_changed", cache_missed_input_tokens }

  

  

  CacheMissToolsChanged { type: "tools_changed", cache_missed_input_tokens }

  

  

  CacheMissUnavailable { type: "unavailable" }

  

  

  CitationCharLocation { type: "char_location", cited_text, document_index, 4
  more }

  

  

  CitationCharLocationParam { type: "char_location", cited_text, document_index,
  3 more }

  

  

  CitationContentBlockLocation { type: "content_block_location", cited_text,
  document_index, 4 more }

  

  

  CitationContentBlockLocationParam { type: "content_block_location",
  cited_text, document_index, 3 more }

  

  

  CitationPageLocation { type: "page_location", cited_text, document_index, 4
  more }

  

  

  CitationPageLocationParam { type: "page_location", cited_text, document_index,
  3 more }

  

  

  CitationSearchResultLocationParam { type: "search_result_location",
  cited_text, end_block_index, 4 more }

  

  

  CitationWebSearchResultLocationParam { type: "web_search_result_location",
  cited_text, encrypted_index, 2 more }

  

  

  CitationsConfig { enabled }

  

  

  CitationsConfigParam { enabled }

  

  

  CitationsDelta { type: "citations_delta", citation }

  

  

  CitationsSearchResultLocation { type: "search_result_location", cited_text,
  end_block_index, 4 more }

  

  

  CitationsWebSearchResultLocation { type: "web_search_result_location",
  cited_text, encrypted_index, 2 more }

  

  

  CodeExecutionOutputBlock { type: "code_execution_output", file_id }

  

  

  CodeExecutionOutputBlockParam { type: "code_execution_output", file_id }

  

  

  CodeExecutionResultBlock { type: "code_execution_result", content,
  return_code, 2 more }

  

  

  CodeExecutionResultBlockParam { type: "code_execution_result", content,
  return_code, 2 more }

  

  

  CodeExecutionTool20250522 { type: "code_execution_20250522", name,
  allowed_callers, 3 more }

  

  

  CodeExecutionTool20250825 { type: "code_execution_20250825", name,
  allowed_callers, 3 more }

  

  

  CodeExecutionTool20260120 { type: "code_execution_20260120", name,
  allowed_callers, 3 more }

  


  Code execution tool with REPL state persistence (daemon mode + gVisor
  checkpoint).


  

  CodeExecutionTool20260521 { type: "code_execution_20260521", name,
  allowed_callers, 3 more }

  


  Code execution tool with REPL state persistence.


  

  CodeExecutionToolResultBlock { type: "code_execution_tool_result", content,
  tool_use_id }

  

  

  CodeExecutionToolResultBlockContent = CodeExecutionToolResultError or
  CodeExecutionResultBlock or EncryptedCodeExecutionResultBlock

  

  

  CodeExecutionToolResultBlockParam { type: "code_execution_tool_result",
  content, tool_use_id, cache_control }

  

  

  CodeExecutionToolResultBlockParamContent = CodeExecutionToolResultErrorParam
  or CodeExecutionResultBlockParam or EncryptedCodeExecutionResultBlockParam

  

  

  CodeExecutionToolResultError { type: "code_execution_tool_result_error",
  error_code }

  

  

  CodeExecutionToolResultErrorCode = "invalid_tool_input" or "unavailable" or
  "too_many_requests" or "execution_time_exceeded"

  

  

  CodeExecutionToolResultErrorParam { type: "code_execution_tool_result_error",
  error_code }

  

  

  ComputerCursorPositionConfig { defer_loading, enabled }

  


  cursor_position's config overrides.


  ComputerCursorPositionInput object{  }

  


  Get the current (x, y) pixel coordinate of the cursor.


  

  ComputerCursorPositionToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerDoubleClickConfig { defer_loading, enabled }

  


  double_click's config overrides.


  

  ComputerDoubleClickInput { coordinate, text }

  


  Double-click the left mouse button at the specified (x, y) pixel coordinate,
  or the current cursor position if coordinate is omitted.


  

  ComputerDoubleClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerHoldKeyConfig { defer_loading, enabled }

  


  hold_key's config overrides.


  

  ComputerHoldKeyInput { duration, text }

  


  Hold down a key or key-combination for a specified duration. Uses the same key
  syntax as key.


  

  ComputerHoldKeyToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerKeyConfig { defer_loading, enabled }

  


  key's config overrides.


  

  ComputerKeyInput { text, repeat }

  


  Press a key or key-combination on the keyboard. Use "+" to combine modifiers
  with a key (e.g. "ctrl+s", "alt+Tab", "ctrl+shift+Escape"). Key names are
  case-insensitive; common names like "Return", "Tab", "Escape", "Up", "Down",
  "Left", "Right", "Home", "End", "Page_Up", "Page_Down", "Delete", "BackSpace"
  are supported.


  

  ComputerKeyToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerLeftClickConfig { defer_loading, enabled }

  


  left_click's config overrides.


  

  ComputerLeftClickDragConfig { defer_loading, enabled }

  


  left_click_drag's config overrides.


  

  ComputerLeftClickDragInput { coordinate, start_coordinate, text }

  


  Click and drag the cursor from start_coordinate to coordinate.


  

  ComputerLeftClickDragToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerLeftClickInput { coordinate, text }

  


  Click the left mouse button at the specified (x, y) pixel coordinate, or the
  current cursor position if coordinate is omitted.


  

  ComputerLeftClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerLeftMouseDownConfig { defer_loading, enabled }

  


  left_mouse_down's config overrides.


  ComputerLeftMouseDownInput object{  }

  


  Press and hold the left mouse button at the current cursor position.


  

  ComputerLeftMouseDownToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerLeftMouseUpConfig { defer_loading, enabled }

  


  left_mouse_up's config overrides.


  ComputerLeftMouseUpInput object{  }

  


  Release the left mouse button.


  

  ComputerLeftMouseUpToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerMiddleClickConfig { defer_loading, enabled }

  


  middle_click's config overrides.


  

  ComputerMiddleClickInput { coordinate, text }

  


  Click the middle mouse button at the specified (x, y) pixel coordinate, or the
  current cursor position if coordinate is omitted.


  

  ComputerMiddleClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerMouseMoveConfig { defer_loading, enabled }

  


  mouse_move's config overrides.


  

  ComputerMouseMoveInput { coordinate }

  


  Move the cursor to a specified (x, y) pixel coordinate. Use this ONLY to hover
  without clicking; otherwise use a click action directly.


  

  ComputerMouseMoveToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerRightClickConfig { defer_loading, enabled }

  


  right_click's config overrides.


  

  ComputerRightClickInput { coordinate, text }

  


  Click the right mouse button at the specified (x, y) pixel coordinate, or the
  current cursor position if coordinate is omitted.


  

  ComputerRightClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerScreenshotConfig { defer_loading, enabled }

  


  screenshot's config overrides.


  ComputerScreenshotInput object{  }

  


  Take a screenshot of the screen.


  

  ComputerScreenshotToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerScrollConfig { defer_loading, enabled }

  


  scroll's config overrides.


  

  ComputerScrollDirection = "up" or "down" or "left" or "right"

  

  

  ComputerScrollInput { scroll_amount, scroll_direction, coordinate, text }

  


  Scroll the screen at the specified (x, y) pixel coordinate, or the current
  cursor position if coordinate is omitted. Do NOT use PageUp/PageDown to
  scroll.


  

  ComputerScrollToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerToolUseBlock = ComputerKeyToolUseBlock or ComputerHoldKeyToolUseBlock
  or ComputerTypeToolUseBlock or 14 more

  

  

  ComputerToolset20260801 { type: "computer_toolset_20260801", cache_control,
  configs }

  


  The computer toolset: a single tools[] entry (carrying no name) that declares
  the computer tool family. The model is served the family's tool with any
  members disabled via configs removed from its schema. Every member is enabled
  by default, zoom included. The single-tool options display_number and
  enable_zoom are not fields of a toolset entry — it carries only type, configs,
  and cache_control; zoom is controlled via configs.zoom.enabled.


  

  ComputerToolsetConfigs { type, cursor_position, double_click, 14 more }

  


  Per-member configuration for computer_toolset_20260801: one optional field per
  member tool, keyed by the member name — the same name the member's tool_use
  blocks carry. Every member is an accepted key, and a member's defaults apply
  wherever its key is absent. Unknown keys are rejected: the field set is this
  toolset version's complete member set.


  

  ComputerTripleClickConfig { defer_loading, enabled }

  


  triple_click's config overrides.


  

  ComputerTripleClickInput { coordinate, text }

  


  Triple-click the left mouse button at the specified (x, y) pixel coordinate,
  or the current cursor position if coordinate is omitted.


  

  ComputerTripleClickToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerTypeConfig { defer_loading, enabled }

  


  type's config overrides.


  

  ComputerTypeInput { text }

  


  Type a string of text on the keyboard.


  

  ComputerTypeToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerWaitConfig { defer_loading, enabled }

  


  wait's config overrides.


  

  ComputerWaitInput { duration }

  


  Wait for a specified duration.


  

  ComputerWaitToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ComputerZoomConfig { defer_loading, enabled }

  


  zoom's config overrides.


  

  ComputerZoomInput { region }

  


  Take a screenshot of a rectangular region. Region coordinates are in the
  full-screenshot space (not physical display pixels). The crop is scaled up to
  fill the image budget so fine details become legible.


  

  ComputerZoomToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  Container { id, expires_at, skills }

  


  Information about the container used in the request (for the code execution
  tool)


  

  ContainerParams { id, skills }

  


  Container parameters with skills to be loaded.


  

  ContainerSkill { type, skill_id, version }

  


  A skill that was loaded in a container (response model).


  

  ContainerUploadBlock { type: "container_upload", file_id }

  


  Response model for a file uploaded to the container.


  

  ContainerUploadBlockParam { type: "container_upload", file_id, cache_control }

  


  A content block that represents a file to be uploaded to the container Files
  uploaded via this block will be available in the container's input directory.


  

  ContentBlock = TextBlock or ThinkingBlock or RedactedThinkingBlock or 9 more

  

  

  ContentBlockParam = TextBlockParam or ImageBlockParam or DocumentBlockParam or
  13 more

  

  

  ContentBlockSource { type: "content", content }

  

  

  ContentBlockSourceContent = TextBlockParam or ImageBlockParam

  

  

  Diagnostics { cache_miss_reason }

  


  Request-level diagnostics: why the prompt cache could not fully reuse the
  prefix of the request named by diagnostics.previous_message_id.


  

  DiagnosticsParam { previous_message_id }

  


  Request-level diagnostics. Currently carries the previous response id for
  prompt-cache divergence reporting.


  

  DirectCaller { type: "direct" }

  


  Tool invocation directly from the model.


  

  DocumentBlock { type: "document", citations, source, title }

  

  

  DocumentBlockParam { type: "document", source, cache_control, 3 more }

  

  

  EncryptedCodeExecutionResultBlock { type: "encrypted_code_execution_result",
  content, encrypted_stdout, 2 more }

  


  Code execution result with encrypted stdout for PFC + web_search results.


  

  EncryptedCodeExecutionResultBlockParam { type:
  "encrypted_code_execution_result", content, encrypted_stdout, 2 more }

  


  Code execution result with encrypted stdout for PFC + web_search results.


  

  FileDocumentSource { type: "file", file_id }

  

  

  FileImageSource { type: "file", file_id }

  

  

  ImageBlockParam { type: "image", source, cache_control, transformations }

  

  

  ImageTransformationsParam { oversized_image }

  


  Configures the transformations the server applies to this image before the
  model observes it. Each key names a condition the server transforms images
  for; its value selects the transformation applied. Omitted keys keep their
  default behavior, and an empty object is equivalent to omitting the field.


  

  InputJSONDelta { type: "input_json_delta", partial_json }

  

  

  JSONOutputFormat { type: "json_schema", schema }

  

  

  MemoryTool20250818 { type: "memory_20250818", name, allowed_callers, 4 more }

  

  

  Message { type: "message", id, container, 8 more }

  

  

  MessageCountTokensTool = Tool or ToolBash20250124 or CodeExecutionTool20250522
  or 18 more

  

  

  MessageCreateParamsContainer = ContainerParams or string

  


  Container identifier for reuse across requests.


  

  MessageDeltaUsage { cache_creation_input_tokens, cache_read_input_tokens,
  input_tokens, 3 more }

  

  

  MessageParam { content, role }

  

  

  MessageTokensCount { input_tokens }

  

  

  Metadata { user_id }

  

  

  Model = "claude-haiku-5-5" or "claude-sonnet-5-5" or "claude-fable-5-1" or 17
  more or string

  


  The model that will complete your prompt.


  

  OutputConfig { effort, format }

  

  

  OutputTokensDetails { thinking_tokens }

  

  

  PlainTextSource { type: "text", data, media_type }

  

  

  RawContentBlockDelta = TextDelta or InputJSONDelta or CitationsDelta or 2 more

  

  

  RawContentBlockDeltaEvent { type: "content_block_delta", delta, index }

  

  

  RawContentBlockStartEvent { type: "content_block_start", content_block,
  index }

  

  

  RawContentBlockStopEvent { type: "content_block_stop", index }

  

  

  RawMessageDeltaEvent { type: "message_delta", delta, usage }

  

  

  RawMessageStartEvent { type: "message_start", message }

  

  

  RawMessageStopEvent { type: "message_stop" }

  

  

  RawMessageStreamEvent = RawMessageStartEvent or RawMessageDeltaEvent or
  RawMessageStopEvent or 3 more

  

  

  RedactedThinkingBlock { type: "redacted_thinking", data }

  

  

  RedactedThinkingBlockParam { type: "redacted_thinking", data }

  

  

  RefusalStopDetails { type: "refusal", category, explanation }

  


  Structured information about a refusal.


  

  SearchResultBlockParam { type: "search_result", content, source, 3 more }

  

  

  ServerToolCaller { type: "code_execution_20250825", tool_id }

  


  Tool invocation generated by a server-side tool.


  

  ServerToolCaller20260120 { type: "code_execution_20260120", tool_id }

  

  

  ServerToolUsage { web_fetch_requests, web_search_requests }

  

  

  ServerToolUseBlock { type: "server_tool_use", id, caller, 2 more }

  

  

  ServerToolUseBlockParam { type: "server_tool_use", id, input, 3 more }

  

  

  SignatureDelta { type: "signature_delta", signature }

  

  

  SkillParams { type, skill_id, version }

  


  Specification for a skill to be loaded in a container (request model).


  

  StopReason = "end_turn" or "max_tokens" or "stop_sequence" or 4 more

  

  

  TextBlock { type: "text", citations, text }

  

  

  TextBlockParam { type: "text", text, cache_control, citations }

  

  

  TextCitation = CitationCharLocation or CitationPageLocation or
  CitationContentBlockLocation or 2 more

  

  

  TextCitationParam = CitationCharLocationParam or CitationPageLocationParam or
  CitationContentBlockLocationParam or 2 more

  

  

  TextDelta { type: "text_delta", text }

  

  

  TextEditorCodeExecutionCreateResultBlock { type:
  "text_editor_code_execution_create_result", is_file_update }

  

  

  TextEditorCodeExecutionCreateResultBlockParam { type:
  "text_editor_code_execution_create_result", is_file_update }

  

  

  TextEditorCodeExecutionStrReplaceResultBlock { type:
  "text_editor_code_execution_str_replace_result", lines, new_lines, 3 more }

  

  

  TextEditorCodeExecutionStrReplaceResultBlockParam { type:
  "text_editor_code_execution_str_replace_result", lines, new_lines, 3 more }

  

  

  TextEditorCodeExecutionToolResultBlock { type:
  "text_editor_code_execution_tool_result", content, tool_use_id }

  

  

  TextEditorCodeExecutionToolResultBlockParam { type:
  "text_editor_code_execution_tool_result", content, tool_use_id,
  cache_control }

  

  

  TextEditorCodeExecutionToolResultError { type:
  "text_editor_code_execution_tool_result_error", error_code, error_message }

  

  

  TextEditorCodeExecutionToolResultErrorCode = "invalid_tool_input" or
  "unavailable" or "too_many_requests" or 2 more

  

  

  TextEditorCodeExecutionToolResultErrorParam { type:
  "text_editor_code_execution_tool_result_error", error_code, error_message }

  

  

  TextEditorCodeExecutionViewResultBlock { type:
  "text_editor_code_execution_view_result", content, file_type, 3 more }

  

  

  TextEditorCodeExecutionViewResultBlockParam { type:
  "text_editor_code_execution_view_result", content, file_type, 3 more }

  

  

  ThinkingBlock { type: "thinking", signature, thinking }

  

  

  ThinkingBlockParam { type: "thinking", signature, thinking }

  

  

  ThinkingConfigAdaptive { type: "adaptive", display }

  

  

  ThinkingConfigBetweenTools { type: "between_tools" }

  

  

  ThinkingConfigDisabled { type: "disabled" }

  

  

  ThinkingConfigEnabled { type: "enabled", budget_tokens, display }

  

  

  ThinkingConfigParam = ThinkingConfigEnabled or ThinkingConfigDisabled or
  ThinkingConfigBetweenTools or ThinkingConfigAdaptive

  


  Configuration for Claude's thinking.


  

  ThinkingDelta { type: "thinking_delta", thinking }

  

  

  Tool { type, input_schema, name, 7 more }

  

  

  ToolBash20250124 { type: "bash_20250124", name, allowed_callers, 4 more }

  

  

  ToolChoice = ToolChoiceAuto or ToolChoiceAny or ToolChoiceTool or
  ToolChoiceNone

  


  How the model should use the provided tools. The model can use a specific
  tool, any available tool, decide by itself, or not use tools at all.


  

  ToolChoiceAny { type: "any", disable_parallel_tool_use }

  


  The model will use any available tools.


  

  ToolChoiceAuto { type: "auto", disable_parallel_tool_use }

  


  The model will automatically decide whether to use tools.


  

  ToolChoiceNone { type: "none" }

  


  The model will not be allowed to use tools.


  

  ToolChoiceTool { type: "tool", name, disable_parallel_tool_use }

  


  The model will use the specified tool with tool_choice.name.


  

  ToolReferenceBlock { type: "tool_reference", tool_name }

  

  

  ToolReferenceBlockParam { type: "tool_reference", tool_name, cache_control }

  


  Tool reference block that can be included in tool_result content.


  

  ToolResultBlockParam { type: "tool_result", tool_use_id, cache_control, 3
  more }

  

  

  ToolSearchToolBm25_20251119 { type, name, allowed_callers, 3 more }

  

  

  ToolSearchToolRegex20251119 { type, name, allowed_callers, 3 more }

  

  

  ToolSearchToolResultBlock { type: "tool_search_tool_result", content,
  tool_use_id }

  

  

  ToolSearchToolResultBlockParam { type: "tool_search_tool_result", content,
  tool_use_id, cache_control }

  

  

  ToolSearchToolResultError { type: "tool_search_tool_result_error", error_code,
  error_message }

  

  

  ToolSearchToolResultErrorCode = "invalid_tool_input" or "unavailable" or
  "too_many_requests" or "execution_time_exceeded"

  

  

  ToolSearchToolResultErrorParam { type: "tool_search_tool_result_error",
  error_code, error_message }

  

  

  ToolSearchToolSearchResultBlock { type: "tool_search_tool_search_result",
  tool_references }

  

  

  ToolSearchToolSearchResultBlockParam { type: "tool_search_tool_search_result",
  tool_references }

  

  

  ToolTextEditor20250124 { type: "text_editor_20250124", name, allowed_callers,
  4 more }

  

  

  ToolTextEditor20250429 { type: "text_editor_20250429", name, allowed_callers,
  4 more }

  

  

  ToolTextEditor20250728 { type: "text_editor_20250728", name, allowed_callers,
  5 more }

  

  

  ToolUnion = Tool or ToolBash20250124 or CodeExecutionTool20250522 or 18 more

  

  

  ToolUseBlock { type: "tool_use", id, caller, 3 more }

  

  

  ToolUseBlockParam { type: "tool_use", id, input, 4 more }

  

  

  ToolUseCaller = DirectCaller or ServerToolCaller or ServerToolCaller20260120

  


  Which party invoked the tool call: the model directly, or a server tool on its
  behalf.


  

  ToolsetToolUseBlock = BrowserToolUseBlock or ComputerToolUseBlock or
  ToolUseBlock

  

  

  URLImageSource { type: "url", url }

  

  

  URLPDFSource { type: "url", url }

  

  

  Usage { cache_creation, cache_creation_input_tokens, cache_read_input_tokens,
  7 more }

  

  

  UserLocation { type: "approximate", city, country, 2 more }

  

  

  WebFetchBlock { type: "web_fetch_result", content, retrieved_at, url }

  

  

  WebFetchBlockParam { type: "web_fetch_result", content, url, retrieved_at }

  

  

  WebFetchTool20250910 { type: "web_fetch_20250910", name, allowed_callers, 9
  more }

  

  

  WebFetchTool20260209 { type: "web_fetch_20260209", name, allowed_callers, 9
  more }

  

  

  WebFetchTool20260309 { type: "web_fetch_20260309", name, allowed_callers, 10
  more }

  


  Web fetch tool with use_cache parameter for bypassing cached content.


  

  WebFetchTool20260318 { type: "web_fetch_20260318", name, allowed_callers, 11
  more }

  

  

  WebFetchToolResultBlock { type: "web_fetch_tool_result", caller, content,
  tool_use_id }

  

  

  WebFetchToolResultBlockParam { type: "web_fetch_tool_result", content,
  tool_use_id, 2 more }

  

  

  WebFetchToolResultErrorBlock { type: "web_fetch_tool_result_error",
  error_code }

  

  

  WebFetchToolResultErrorBlockParam { type: "web_fetch_tool_result_error",
  error_code }

  

  

  WebFetchToolResultErrorCode = "invalid_tool_input" or "url_too_long" or
  "url_not_allowed" or 7 more

  

  

  WebFetchURLSourceAll { type: "all" }

  


  The url_sources variant under which a source contributes in full: every result
  of the tool filter's source, or all user input.


  

  WebFetchURLSourceExcept { type: "except", tools }

  


  The tool filter variant under which every result but the named tools'
  contributes.


  

  WebFetchURLSourceNone { type: "none" }

  


  The url_sources variant under which a source contributes nothing: no result of
  the tool filter's source, or no user input.


  

  WebFetchURLSourceOnly { type: "only", tools }

  


  The tool filter variant under which only the named tools' results contribute.


  

  WebFetchURLSourceToolReference { type: "tool_reference", name }

  


  One entry of a tool filter's tools: it must name a tool declared in this
  request's tools[].


  

  WebFetchURLSources { client_tool_results, server_tool_results, user_input }

  


  Which sources contribute to the set of URLs web fetch may fetch.


  

  WebSearchResultBlock { type: "web_search_result", encrypted_content, page_age,
  2 more }

  

  

  WebSearchResultBlockParam { type: "web_search_result", encrypted_content,
  title, 2 more }

  

  

  WebSearchTool20250305 { type: "web_search_20250305", name, allowed_callers, 7
  more }

  

  

  WebSearchTool20260209 { type: "web_search_20260209", name, allowed_callers, 7
  more }

  

  

  WebSearchTool20260318 { type: "web_search_20260318", name, allowed_callers, 8
  more }

  

  

  WebSearchToolRequestError { type: "web_search_tool_result_error", error_code }

  

  

  WebSearchToolResultBlock { type: "web_search_tool_result", caller, content,
  tool_use_id }

  

  

  WebSearchToolResultBlockContent = WebSearchToolResultError or array of
  WebSearchResultBlock

  

  

  WebSearchToolResultBlockParam { type: "web_search_tool_result", content,
  tool_use_id, 2 more }

  

  

  WebSearchToolResultBlockParamContent = array of WebSearchResultBlockParam or
  WebSearchToolRequestError

  

  

  WebSearchToolResultError { type: "web_search_tool_result_error", error_code }

  

  

  WebSearchToolResultErrorCode = "invalid_tool_input" or "unavailable" or
  "max_uses_exceeded" or 3 more

  

  Messages

  

  Batches

  Create a Message Batch

  

  POST

  /v1/messages/batches


  Send a batch of Message creation requests.


  Retrieve a Message Batch

  

  GET

  /v1/messages/batches/{message_batch_id}


  This endpoint is idempotent and can be used to poll for Message Batch
  completion. To access the results of a Message Batch, make a request to the
  results_url field in the response.


  List Message Batches

  

  GET

  /v1/messages/batches


  List all Message Batches within a Workspace. Most recently created batches are
  returned first.


  Cancel a Message Batch

  

  POST

  /v1/messages/batches/{message_batch_id}/cancel


  Batches may be canceled any time before processing ends. Once cancellation is
  initiated, the batch enters a canceling state, at which time the system may
  complete any in-progress, non-interruptible requests before finalizing
  cancellation.


  Delete a Message Batch

  

  DELETE

  /v1/messages/batches/{message_batch_id}

  Retrieve Message Batch results

  

  GET

  /v1/messages/batches/{message_batch_id}/results


  Streams the results of a Message Batch as a .jsonl file.


  Claude Platform Docs

  

  

  Solutions

  AI agents

  Code modernization

  Coding

  Customer support

  Financial services

  Government

  Higher education

  K-12 teachers

  Life sciences

  Partners

  Claude on AWS

  Claude on Google Cloud

  Learn

  Blog

  Courses

  Use cases

  Connectors

  Customer stories

  Engineering at Anthropic

  Events

  Powered by Claude

  Service partners

  Startups program

  Company

  Anthropic

  Careers

  Economic Futures

  Research

  News

  Responsible Scaling Policy

  Security and compliance

  Transparency

  Help and security

  Availability

  Status

  Support

  Discord

  Terms and policies

  Privacy policy

  Responsible disclosure policy

  Terms of service: Commercial

  Terms of service: Consumer

  Usage policy

  Ask Docs

  
