# FrostPass

This project is a decentralized ticketing protocol built on Solana using Metaplex Core to
eliminate ticket scalping and entry fraud. The system sets a hardcoded resale price cap
(maximum 10% to 20% above face value) and limits each ticket to a maximum of two lifetime
transfers. Direct person-to-person transfers are blocked on-chain. If an attendee cannot attend,
they must deposit their ticket into an automated, anonymous matching queue that sells it to
the next buyer in line. To prevent scalpers from selling wallet private keys for cash, venue
check-in uses dynamic, rotating QR codes linked to the app rather than a static wallet balance.
