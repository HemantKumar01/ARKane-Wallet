"use client"
import { Button } from "@/components/ui/button"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"

interface CreateWalletFormProps {
  isLoading: boolean
  setIsLoading: (loading: boolean) => void
  onWalletCreated: (walletId: string, onchainAddress: string, offchainAddress: string) => void
}

export function CreateWalletForm({ isLoading, setIsLoading, onWalletCreated }: CreateWalletFormProps) {
  const handleCreateWallet = async () => {
    try {
      setIsLoading(true)

      // Create wallet
      const createResponse = await fetch("http://localhost:8080/create_wallet", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
      })

      if (!createResponse.ok) {
        throw new Error("Failed to create wallet")
      }

      const createData = await createResponse.json()
      const walletId = createData.wallet_id

      // Get addresses
      const addressResponse = await fetch(`http://localhost:8080/get_address/${walletId}`, {
        method: "GET",
      })

      if (!addressResponse.ok) {
        throw new Error("Failed to get addresses")
      }

      const addressData = await addressResponse.json()
      const { onchain_address, offchain_address } = addressData

      onWalletCreated(walletId, onchain_address, offchain_address)
    } catch (error) {
      console.error("Error creating wallet:", error)
      toast.error("Error", {
        description: error instanceof Error ? error.message : "Failed to create wallet",
      })
    } finally {
      setIsLoading(false)
    }
  }

  return (
    <div className="flex flex-col space-y-6">
      <p className="text-sm text-muted-foreground">
        Create a new Bitcoin wallet to start sending and receiving payments.
      </p>

      <Button onClick={handleCreateWallet} disabled={isLoading} className="bg-orange-600 hover:bg-orange-700">
        {isLoading ? (
          <>
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            Creating Wallet...
          </>
        ) : (
          "Create New Wallet"
        )}
      </Button>
    </div>
  )
}
