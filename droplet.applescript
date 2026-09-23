on open theFiles
	set script_path to (POSIX path of (path to me)) & "Contents/Resources/bitonalpdf.sh"
	set env to "export PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin; "
	set textChoice to "Pure text (1-bit, smallest)"
	set valg to choose from list {textChoice, "With pictures/diagrams (colour)"} with title "bitonalPDF" with prompt "What kind of PDF?" default items {textChoice}
	if valg is false then return
	set m to "text"
	if (item 1 of valg) starts with "With" then set m to "images"

	repeat with f in theFiles
		set p to POSIX path of f
		if p ends with ".pdf" then
			try
				set total to (do shell script env & "pdfinfo " & quoted form of p & " | awk '/^Pages:/ {print $2}'") as integer
				set pf to do shell script "mktemp"
				set resf to do shell script "mktemp"
				set progress total steps to total
				set progress completed steps to 0
				set progress description to "Shrinking " & (do shell script "basename " & quoted form of p)
				set pid to do shell script env & "MODE=" & m & " PROGRESS_FILE=" & quoted form of pf & " " & quoted form of script_path & " " & quoted form of p & " > " & quoted form of resf & " 2>&1 & echo $!"
				repeat
					delay 1
					set done_n to (do shell script "wc -l < " & quoted form of pf) as integer
					set progress completed steps to done_n
					if done_n ≥ total then set progress additional description to "Assembling PDF…"
					if (do shell script "kill -0 " & pid & " 2>/dev/null && echo 1 || echo 0") is "0" then exit repeat
				end repeat
				set r to do shell script "tail -1 " & quoted form of resf
				do shell script "rm -f " & quoted form of pf & " " & quoted form of resf
				display notification r with title "bitonalPDF"
			on error e
				display notification e with title "bitonalPDF failed"
			end try
		end if
	end repeat
	set progress total steps to 0
end open

on run
	display dialog "Drop a scanned PDF onto this icon." buttons {"OK"}
end run
