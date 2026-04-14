# Minor issues tracker

This file contains minor issues found in the application while testing, but not seen important enough to describe and fix right away.  

In many places we show the full path where we keep managed things. For example, where the LSPs are saved (Application Support folder). The same thing happens for where we keep the managed git checkout. We should never show those, it doesn't concern the user where they are stored. We can show one the storage folder once in the Settings view but nowhere else.

The heading bar where pull request name and other metadata is shown takes too much space when actually viewing files or code tour. It can be like that when you navigate to the pull request, but should animate to take less space when scrolling down and viewing the files.

When hovering for code tips from LSP, if the returned result from LSP contains markdown and bold text, it doesn't fit the UI box it is put into. Also the box is not big enough if the text is long.

The current review state should be shown more easily: who has approved, how hasn't viewed yet, who requested changes. Not after the description, maybe in the side.

When viewing your own pull request, the comments you receive should be summaried somewhere so you see all of them easily AND in the code like they are now. And you should be able to click to open the file where the comment is easily. In essence it should be as easy as possible to see information about your own pull request and what feedback you have gotten.
