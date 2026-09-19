on run argv
  set wanted to item 2 of argv
  set targetPid to item 3 of argv as integer
  tell application "System Events"
    set p to first application process whose unix id is targetPid
    tell p
      set matches to every window whose name is wanted
      if (count of matches) is 1 then
        set frontmost to true
        perform action "AXRaise" of item 1 of matches
        return "focused"
      end if
    end tell
  end tell
  return "missing"
end run
