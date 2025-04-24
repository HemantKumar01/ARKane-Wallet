"use client"

import { useState } from "react"
import { useRouter } from "next/navigation"
import { Bitcoin } from "lucide-react"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { CreateWalletForm } from "./create-wallet-form"
import { RestoreWalletForm } from "./restore-wallet-form"
import { toast } from "sonner"

export function LandingPage() {
  const [isLoading, setIsLoading] = useState(false)
  const router = useRouter()

  const handleWalletCreated = (walletId: string, onchainAddress: string, offchainAddress: string) => {
    // Store wallet info in localStorage
    localStorage.setItem("wallet_id", walletId)
    localStorage.setItem("onchain_address", onchainAddress)
    localStorage.setItem("offchain_address", offchainAddress)

    toast.success("Wallet Created", {
      description: "Your wallet has been successfully created.",
    })

    router.push("/wallet")
  }

  const handleWalletRestored = (walletId: string, onchainAddress: string, offchainAddress: string) => {
    // Store wallet info in localStorage
    localStorage.setItem("wallet_id", walletId)
    localStorage.setItem("onchain_address", onchainAddress)
    localStorage.setItem("offchain_address", offchainAddress)

    toast.success("Wallet Restored", {
      description: "Your wallet has been successfully restored.",
    })

    router.push("/wallet")
  }

  return (
    <div className="flex min-h-screen flex-col items-center justify-center bg-background p-4">
      <div className="flex items-center mb-8 space-x-2">
        <Bitcoin className="h-10 w-10 text-orange-500" />
        <h1 className="text-3xl font-bold">ARKane Wallet</h1>
      </div>

      <Card className="w-full max-w-md border-zinc-800 bg-zinc-950">
        <CardHeader>
          <CardTitle className="text-2xl text-center">Welcome to ARKane</CardTitle>
          <CardDescription className="text-center">A modern Bitcoin wallet for the next generation</CardDescription>
        </CardHeader>
        <CardContent>
          <Tabs defaultValue="create" className="w-full">
            <TabsList className="grid w-full grid-cols-2 mb-6">
              <TabsTrigger value="create">Create Wallet</TabsTrigger>
              <TabsTrigger value="restore">Restore Wallet</TabsTrigger>
            </TabsList>
            <TabsContent value="create">
              <CreateWalletForm
                isLoading={isLoading}
                setIsLoading={setIsLoading}
                onWalletCreated={handleWalletCreated}
              />
            </TabsContent>
            <TabsContent value="restore">
              <RestoreWalletForm
                isLoading={isLoading}
                setIsLoading={setIsLoading}
                onWalletRestored={handleWalletRestored}
              />
            </TabsContent>
          </Tabs>
        </CardContent>
      </Card>
    </div>
  )
}
