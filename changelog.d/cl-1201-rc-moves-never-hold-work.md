A dev home's rentals move to the open RC only while idle, and never ahead of work (cl-1201). A submit to a rental not on the RC
used to fetch the RC's cozy from GitHub inside the request (49 s on a laptop) and move the rental first; now the RC's cozy is
fetched while the rental boots, the run goes at once on what the rental runs, and the rental moves once it is idle. A render that
comes while the follower's move is still being sent no longer waits behind its restart: the move yields and goes again at idle.
