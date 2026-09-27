-- Read-only Ghostty lookups for notification_origin.py. Arguments arrive through
-- argv and are only compared with Ghostty's values; they are never executed.
--   titles            -> one "<terminal id><tab><title>" line per terminal
--   title <marker>    -> window, tab and terminal IDs of the terminal so titled
--   terminal <id>     -> window, tab and terminal IDs of that terminal
on run argv
  set mode to item 1 of argv
  -- Inside the tell block, "tab" names Ghostty's tab class, not the character.
  set separator to character id 9
  tell application "Ghostty"
    if mode is "titles" then
      set out to {}
      repeat with t in terminals
        set end of out to (id of t as text) & separator & (name of t as text)
      end repeat
      set AppleScript's text item delimiters to linefeed
      return out as text
    end if
    set wanted to item 2 of argv
    set found to {}
    repeat with w in windows
      repeat with tb in tabs of w
        repeat with t in terminals of tb
          if (mode is "title" and (name of t as text) is wanted) or (mode is "terminal" and (id of t as text) is wanted) then
            set end of found to (id of w as text) & linefeed & (id of tb as text) & linefeed & (id of t as text)
          end if
        end repeat
      end repeat
    end repeat
  end tell
  -- An ambiguous match identifies nothing.
  if (count of found) is 1 then return item 1 of found
  return ""
end run
